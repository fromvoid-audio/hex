use super::EditorView;
use gpui::{AppContext, Context, PathPromptOptions, Window};
pub(super) mod project_open;
use hex_project::TrackId;
use hex_session::service::Request;
use hex_session::snapshot_exports::{SnapshotRead, SnapshotTransportError};

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
                        view.update(cx, |view, cx| handler(view, window, cx));
                    }
                });
            });
            cx.stop_propagation();
        });
    }
    register::<super::NewProject>(cx, |view, _, cx| {
        view.new_project(hex_session::DocumentAudioMode::Resident);
        cx.notify();
    });
    register::<super::NewStreamingProject>(cx, |view, _, cx| {
        view.new_project(hex_session::DocumentAudioMode::VerifiedStreaming);
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
    pub(super) fn dialog_snapshot<R>(
        &self,
        inspect: impl for<'read> FnOnce(SnapshotRead<'read>) -> R,
    ) -> Result<Option<R>, SnapshotTransportError> {
        self.state
            .as_ref()
            .map(|state| state.read_with(inspect))
            .transpose()
    }

    pub(super) fn dialog_snapshot_ready(&mut self) -> bool {
        match self.dialog_snapshot(|_| ()) {
            Ok(Some(())) => true,
            Ok(None) => {
                self.error = Some("project is still loading".into());
                false
            }
            Err(SnapshotTransportError::Busy) => {
                self.error = Some("project read busy · try again".into());
                false
            }
            Err(_) => {
                self.error = Some("project read is unavailable".into());
                false
            }
        }
    }

    pub(super) fn open_dialog(
        &mut self,
        mode: hex_session::DocumentAudioMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.dialog_snapshot_ready() {
            cx.notify();
            return;
        }
        match super::device_panel::finalize_controls_checked(self, cx) {
            Ok(true) => {}
            Ok(false) => {
                cx.notify();
                return;
            }
            Err(error) => {
                self.error = Some(if error == SnapshotTransportError::Busy {
                    "project read busy · try opening again".into()
                } else {
                    "project read is unavailable".into()
                });
                cx.notify();
                return;
            }
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
                    prompt: Some(match mode {
                        hex_session::DocumentAudioMode::Resident => "open project".into(),
                        hex_session::DocumentAudioMode::VerifiedStreaming => {
                            "open project as verified streaming".into()
                        }
                    }),
                },
                directory.as_deref(),
            );
            cx.spawn(async move |this, cx| {
                let mut picked = Some(response.await);
                loop {
                    let retry = cx.update_window(handle, |_, window, cx| {
                        this.update(cx, |view, cx| {
                            if !view.open_picker_is_current(generation, serial) {
                                view.finish_open_picker(generation, serial);
                                return false;
                            }
                            let has_path = matches!(picked.as_ref(), Some(Ok(Ok(Some(paths)))) if !paths.is_empty());
                            let finalized = if has_path {
                                match super::device_panel::finalize_controls_checked(view, cx) {
                                    Ok(finalized) => finalized,
                                    Err(SnapshotTransportError::Busy) => return true,
                                    Err(_) => {
                                        view.error = Some("project read is unavailable".into());
                                        false
                                    }
                                }
                            } else { false };
                            if !view.finish_open_picker(generation, serial) {
                                return false;
                            }
                            if window.focused(cx).is_none() {
                                view.focus.focus(window);
                            }
                            match picked.take() {
                                Some(Ok(Ok(Some(mut paths)))) if finalized && !paths.is_empty() => {
                                    if let Some(path) = paths.pop() {
                                        view.choose_project(path, mode);
                                    }
                                }
                                Some(Ok(Err(error))) => {
                                    view.error = Some(error.to_string().to_lowercase().into())
                                }
                                Some(Ok(Ok(None))) => view.status = "open cancelled".into(),
                                Some(Ok(Ok(Some(paths)))) if paths.is_empty() => {
                                    view.status = "open cancelled".into()
                                }
                                Some(Err(_)) => view.status = "open cancelled".into(),
                                _ => {}
                            }
                            cx.notify();
                            false
                        }).unwrap_or(false)
                    }).unwrap_or(false);
                    if !retry { break; }
                    cx.background_executor().timer(std::time::Duration::from_millis(5)).await;
                }
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
            || self.gain_recorder.blocks_edits()
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

    fn begin_open(&mut self, path: std::path::PathBuf, mode: hex_session::DocumentAudioMode) {
        let request = match mode {
            hex_session::DocumentAudioMode::Resident => Request::Open(path),
            hex_session::DocumentAudioMode::VerifiedStreaming => {
                let Some(reservation) = self.source_reservation else {
                    self.error = Some("verified streaming is unavailable".into());
                    return;
                };
                Request::OpenWithAudioMode {
                    source: hex_session::service::ProjectSource::Saved(path),
                    relinks: std::collections::BTreeMap::new(),
                    mode,
                    reservation: Some(reservation),
                }
            }
        };
        self.begin_open_request(request);
    }

    pub(super) fn new_project(&mut self, mode: hex_session::DocumentAudioMode) {
        if self.opening_project || self.project_open.busy() || self.pending_open_path.is_some() {
            return;
        }
        let has_content = match self.dialog_snapshot(|snapshot| {
            snapshot.unsaved()
                || snapshot.model().tracks().len() != 0
                || snapshot.model().has_assets()
        }) {
            Ok(Some(has_content)) => has_content,
            Ok(None) => false,
            Err(SnapshotTransportError::Busy) => {
                self.error = Some("project read busy · choose new project again".into());
                return;
            }
            Err(_) => {
                self.error = Some("project read is unavailable".into());
                return;
            }
        };
        let has_content = has_content || self.tuning.client.snapshot().is_some();
        if has_content && self.pending_new_mode != Some(mode) {
            self.pending_new_mode = Some(mode);
            self.status = "replace project · choose new project again to confirm".into();
            self.error = None;
            return;
        }
        self.pending_new_mode = None;
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
        let track = match self.dialog_snapshot(|snapshot| {
            (self.mode == super::EditorMode::Arrangement)
                .then_some(self.selected_track)
                .flatten()
                .filter(|track| snapshot.model().track(*track).is_some())
        }) {
            Ok(Some(track)) => track,
            Ok(None) => return,
            Err(SnapshotTransportError::Busy) => {
                self.error = Some("project read busy · try importing again".into());
                cx.notify();
                return;
            }
            Err(_) => {
                self.error = Some("project read is unavailable".into());
                return;
            }
        };
        let destination = ImportPromptTarget {
            track,
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
                    Ok(Err(error)) => view.error = Some(error.to_string().to_lowercase().into()),
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
            let mut picked = Some(response.await);
            loop {
                let retry = this.update(cx, |view, cx| {
                    let has_path = matches!(picked.as_ref(), Some(Ok(Ok(Some(paths)))) if !paths.is_empty());
                    let target_valid = if !has_path || view.project_generation != project_generation {
                        Some(false)
                    } else {
                        match view.dialog_snapshot(|snapshot| {
                            let project = snapshot.model();
                            project.track(track).is_some()
                                && project.scenes().any(|item| item.id() == scene)
                                && matches!(project.session_cell(track, scene), hex_session::snapshot_exports::ui_consumers::SnapshotSessionCellContentRead::Empty)
                        }) {
                            Ok(value) => Some(value.unwrap_or(false)),
                            Err(SnapshotTransportError::Busy) => return true,
                            Err(_) => {
                                view.error = Some("project read is unavailable".into());
                                cx.notify();
                                return false;
                            }
                        }
                    };
                    let Some(picked) = picked.take() else { return false; };
                    match picked {
                    Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                        if let Some(path) = paths.pop() {
                            if target_valid != Some(true) {
                                view.error = Some(
                                    "select an empty audio session cell before importing".into(),
                                );
                            } else {
                                view.enqueue(Request::ImportToSession { path, track, scene });
                            }
                        }
                    }
                    Ok(Err(error)) => view.error = Some(error.to_string().to_lowercase().into()),
                    _ => {}
                }
                    cx.notify();
                    false
                }).unwrap_or(false);
                if !retry { break; }
                cx.background_executor().timer(std::time::Duration::from_millis(5)).await;
            }
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
