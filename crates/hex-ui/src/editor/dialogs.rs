use super::EditorView;
use gpui::{AppContext, Context, PathPromptOptions, Window};
pub(super) mod project_open;
use hex_project::TrackId;
use hex_session::service::Request;

#[derive(Clone, Copy)]
struct ImportPromptTarget {
    track: Option<TrackId>,
    start: u64,
    project_generation: u64,
}

impl ImportPromptTarget {
    const fn belongs_to(self, project_generation: u64) -> bool {
        self.project_generation == project_generation
    }
}

struct ProjectActionsInstalled;
impl gpui::Global for ProjectActionsInstalled {}

/// Focused editor handlers run first. These four File menu fallbacks resolve the
/// active window at dispatch time, including when its element focus was lost.
pub(super) fn install_project_actions(cx: &mut gpui::App) {
    if cx.has_global::<ProjectActionsInstalled>() {
        return;
    }
    cx.set_global(ProjectActionsInstalled);
    fn register<A: gpui::Action>(
        cx: &mut gpui::App,
        handler: fn(&mut EditorView, &mut Window, &mut Context<EditorView>),
    ) {
        cx.on_action(move |_: &A, cx| {
            let Some(window) = cx.active_window() else {
                cx.propagate();
                return;
            };
            // The dispatching window is currently borrowed by GPUI. Resolve its
            // root once that dispatch completes, keeping this exact window ID.
            cx.defer(move |cx| {
                let _ = window.update(cx, |_, window, cx| {
                    if let Some(Some(view)) = window.root::<EditorView>() {
                        view.update(cx, |view, cx| {
                            let _scope = crate::diagnostics::action_scope(cx, A::name_for_type());
                            handler(view, window, cx);
                        });
                    }
                });
            });
            cx.stop_propagation();
        });
    }
    register::<super::NewProject>(cx, |view, window, cx| {
        view.new_project(hex_session::DocumentAudioMode::Resident, window, cx);
        cx.notify();
    });
    register::<super::NewStreamingProject>(cx, |view, window, cx| {
        view.new_project(
            hex_session::DocumentAudioMode::VerifiedStreaming,
            window,
            cx,
        );
        cx.notify();
    });
    register::<super::OpenProject>(cx, |view, window, cx| {
        view.open_dialog(hex_session::DocumentAudioMode::Resident, window, cx);
    });
    register::<super::OpenStreamingProject>(cx, |view, window, cx| {
        view.open_dialog(
            hex_session::DocumentAudioMode::VerifiedStreaming,
            window,
            cx,
        );
    });
}

impl EditorView {
    pub(super) fn open_dialog(
        &mut self,
        mode: hex_session::DocumentAudioMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        if mode == hex_session::DocumentAudioMode::VerifiedStreaming
            && self.source_reservation.is_none()
        {
            self.error = Some("verified streaming is unavailable".into());
            cx.notify();
            return;
        }
        // Native pickers retain logical text focus. Finish the existing
        // blur/Enter rename before Open admission blocks ordinary edits; the
        // selected-path reconciliation already waits for its canonical worker.
        if !self.finalize_track_name_for_open(window, cx)
            || !self.finalize_clip_name_for_open(window, cx)
            || !self.finalize_section_name_for_open(window, cx)
        {
            cx.notify();
            return;
        }
        let Some((generation, serial)) = self.start_open_picker() else {
            cx.notify();
            return;
        };
        let handle = window.window_handle();
        self.with_project_picker_directory(cx, move |view, directory, cx| {
            if !view.open_picker_is_current(generation, serial) {
                return;
            }
            if !cx.windows().contains(&handle) {
                view.finish_open_picker(generation, serial);
                return;
            }
            let response = cx.prompt_for_paths_with_directory(
                PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some("open project".into()),
                },
                directory.as_deref(),
            );
            cx.spawn(async move |this, cx| {
                let picked = response.await;
                let _ = cx.update_window(handle, |_, window, cx| {
                    let _ = this.update(cx, |view, cx| {
                        if !view.finish_open_picker(generation, serial) {
                            return;
                        }
                        if window.focused(cx).is_none() {
                            view.focus.focus(window);
                        }
                        match picked {
                            Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                                if let Some(path) = paths.pop()
                                    && super::device_panel::finalize_controls(view, cx)
                                {
                                    view.choose_project(path, mode);
                                }
                            }
                            Ok(Err(error)) => {
                                view.error = Some(
                                    hex_session::service::text::lowercase_outside_verbatim(
                                        &error.to_string(),
                                    )
                                    .into(),
                                )
                            }
                            _ => view.status = "open cancelled".into(),
                        }
                        cx.notify();
                    });
                });
            })
            .detach();
        });
    }

    pub(super) fn choose_project(
        &mut self,
        path: std::path::PathBuf,
        mode: hex_session::DocumentAudioMode,
    ) {
        if path.as_os_str().len() > 4_096 {
            self.error = Some("project path exceeds the 4096-byte limit".into());
            return;
        }
        if self.opening_project
            || self.project_open.busy()
            || self.close_prompt_open
            || self.saving_project
            || self.save_dialog_open
            || (self.gain_recorder.blocks_edits() || self.midi_recorder.blocks_edits())
        {
            self.error =
                Some("finish the current project action before opening another project".into());
            return;
        }
        if self.state.is_none() {
            self.error = Some("project is still loading".into());
            return;
        }
        self.pending_open_path = Some(path);
        self.pending_open_mode = Some(mode);
        self.status = "opening selected project".into();
        self.error = None;
    }

    /// Opens after a read-only missing-media check; missing files show the
    /// relink panel instead of failing the open.
    fn begin_open(
        &mut self,
        path: std::path::PathBuf,
        mode: hex_session::DocumentAudioMode,
        cx: &mut Context<Self>,
    ) {
        let source = hex_session::service::ProjectSource::Saved(path.clone());
        let (request, reservation) = match mode {
            hex_session::DocumentAudioMode::Resident => (Request::Open(path), None),
            hex_session::DocumentAudioMode::VerifiedStreaming => {
                let Some(reservation) = self.source_reservation else {
                    self.error = Some("verified streaming is unavailable".into());
                    return;
                };
                (
                    Request::OpenWithAudioMode {
                        source: source.clone(),
                        relinks: std::collections::BTreeMap::new(),
                        mode,
                        reservation: Some(reservation),
                    },
                    Some(reservation),
                )
            }
        };
        self.open_checking_missing_media(source, mode, reservation, request, cx);
    }

    pub(super) fn new_project(
        &mut self,
        mode: hex_session::DocumentAudioMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.opening_project
            || self.project_open.busy()
            || self.pending_open_path.is_some()
            || self.close_prompt_open
            || self.saving_project
            || self.save_dialog_open
            || (self.gain_recorder.blocks_edits() || self.midi_recorder.blocks_edits())
        {
            return;
        }
        if mode == hex_session::DocumentAudioMode::VerifiedStreaming
            && self.source_reservation.is_none()
        {
            self.error = Some("verified streaming is unavailable".into());
            return;
        }
        if !super::device_panel::finalize_controls(self, cx)
            || !self.finalize_track_name_for_open(window, cx)
            || !self.finalize_clip_name_for_open(window, cx)
            || !self.finalize_section_name_for_open(window, cx)
        {
            return;
        }
        self.queue_new_project(mode);
    }

    fn begin_new_project(&mut self, mode: hex_session::DocumentAudioMode) {
        let Some(reservation) = self.source_reservation else {
            if mode == hex_session::DocumentAudioMode::VerifiedStreaming {
                self.error = Some("verified streaming is unavailable".into());
                return;
            }
            self.begin_open_request(Request::NewWithAudioMode {
                mode,
                reservation: None,
            });
            return;
        };
        self.begin_open_request(Request::NewWithAudioMode {
            mode,
            reservation: (mode == hex_session::DocumentAudioMode::VerifiedStreaming)
                .then_some(reservation),
        });
    }

    pub(super) fn begin_open_request(&mut self, request: Request) {
        if self.project_open.busy() {
            self.error =
                Some("finish the current project action before replacing the project".into());
            return;
        }
        let requested_mode = match &request {
            Request::NewWithAudioMode { mode, .. } | Request::OpenWithAudioMode { mode, .. } => {
                Some(*mode)
            }
            _ => None,
        };
        self.pending_open_path = None;
        self.pending_open_mode = None;
        self.cancel_duplicate_chain();
        if self.enqueue(request) {
            self.requested_audio_mode = requested_mode;
            self.opening_project = true;
            self.pending_audio_commands.clear();
            self.pending_seek_ticket = None;
            self.pending_session_ticket = None;
            self.play_intent.clear();
            self.status = "opening project · editing paused".into();
        }
    }

    pub(super) fn import_dialog(&mut self, cx: &mut Context<Self>) {
        let destination = ImportPromptTarget {
            track: (self.mode == super::EditorMode::Arrangement)
                .then_some(self.selected_track)
                .flatten()
                .filter(|track| {
                    self.state
                        .as_ref()
                        .is_some_and(|state| state.project.track(*track).is_some())
                }),
            start: self.cursor_position(),
            project_generation: self.project_generation,
        };
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("import audio".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = response.await;
            let _ = this.update(cx, |view, cx| {
                if !destination.belongs_to(view.project_generation) {
                    view.status = "audio import cancelled because project changed".into();
                    cx.notify();
                    return;
                }
                match picked {
                    Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                        if let Some(path) = paths.pop() {
                            if let Some(track) = destination.track {
                                view.enqueue(Request::ImportToTrack {
                                    path,
                                    track,
                                    start: destination.start,
                                });
                            } else {
                                view.enqueue(Request::Import(path));
                            }
                        }
                    }
                    Ok(Err(error)) => {
                        view.error = Some(
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string(),
                            )
                            .into(),
                        )
                    }
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn import_audio_to_session_dialog(
        &mut self,
        track: hex_project::TrackId,
        scene: hex_project::SceneId,
        cx: &mut Context<Self>,
    ) {
        let project_generation = self.project_generation;
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("import audio source to session · one-shot".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = response.await;
            let _ = this.update(cx, |view, cx| {
                match picked {
                    Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                        if let Some(path) = paths.pop() {
                            let target_valid = view.project_generation == project_generation
                                && view.state.as_ref().is_some_and(|state| {
                                    state.project.track(track).is_some()
                                        && state
                                            .project
                                            .scenes()
                                            .iter()
                                            .any(|item| item.id() == scene)
                                        && state.project.session_cell(track, scene).is_none()
                                });
                            if !target_valid {
                                view.error = Some(
                                    "select an empty audio session cell before importing".into(),
                                );
                            } else {
                                view.enqueue(Request::ImportToSession { path, track, scene });
                            }
                        }
                    }
                    Ok(Err(error)) => {
                        view.error = Some(
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string(),
                            )
                            .into(),
                        )
                    }
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::ImportPromptTarget;
    use hex_project::{Command, Created, Editor, Project};

    #[test]
    fn pending_import_rejects_same_numeric_track_id_from_a_new_project() {
        let mut original = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
        let original_track = match original
            .execute(Command::AddTrack {
                name: "original".into(),
            })
            .expect("original track")
        {
            Created::Track(track) => track,
            other => panic!("unexpected track: {other:?}"),
        };
        let mut replacement =
            Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
        let replacement_track = match replacement
            .execute(Command::AddTrack {
                name: "replacement".into(),
            })
            .expect("replacement track")
        {
            Created::Track(track) => track,
            other => panic!("unexpected track: {other:?}"),
        };
        assert_eq!(original_track, replacement_track);

        let pending = ImportPromptTarget {
            track: Some(original_track),
            start: 128,
            project_generation: 9,
        };
        assert!(!pending.belongs_to(10));
        assert_eq!(pending.track, Some(original_track));
        assert_eq!(pending.start, 128);
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod ui_tests;
