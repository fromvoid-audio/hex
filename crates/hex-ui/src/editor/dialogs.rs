use super::EditorView;
use gpui::{Context, PathPromptOptions};
use hex_project::TrackId;
use hex_session::service::Request;
use hex_session::snapshot_exports::SnapshotTransportError;
use hex_session::snapshot_exports::ui_consumers::SnapshotSessionCellContentRead;

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

impl EditorView {
    pub(super) fn open_dialog(
        &mut self,
        mode: hex_session::DocumentAudioMode,
        cx: &mut Context<Self>,
    ) {
        if self.project_replace_confirmation().is_err() {
            self.error = Some("project read unavailable · cannot open project".into());
            return;
        }
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        if self.opening_project {
            return;
        }
        if let Some(path) = self.pending_open_path.take() {
            self.begin_open(path, mode);
            cx.notify();
            return;
        }
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(match mode {
                hex_session::DocumentAudioMode::Resident => "open project".into(),
                hex_session::DocumentAudioMode::VerifiedStreaming => {
                    "open project as verified streaming".into()
                }
            }),
        });
        cx.spawn(async move |this, cx| {
            let picked = response.await;
            let _ = this.update(cx, |view, cx| {
                match picked {
                    Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                        if let Some(path) = paths.pop() {
                            match view.project_replace_confirmation() {
                                Ok(Some(needs_confirmation))
                                    if super::device_panel::finalize_controls(view, cx) =>
                                {
                                    view.choose_project_prepared(path, mode, needs_confirmation)
                                }
                                Err(_) => {
                                    view.error = Some(
                                        "project read unavailable · cannot open project".into(),
                                    )
                                }
                                _ => {}
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

    pub(super) fn choose_project(
        &mut self,
        path: std::path::PathBuf,
        mode: hex_session::DocumentAudioMode,
    ) {
        if path.as_os_str().len() > 4_096 {
            self.error = Some("project path exceeds the 4096-byte limit".into());
            return;
        }
        let needs_confirmation = match self.project_replace_confirmation() {
            Ok(Some(needs_confirmation)) => needs_confirmation,
            Ok(None) => {
                self.error = Some("project is still loading".into());
                return;
            }
            Err(_) => {
                self.error = Some("project read unavailable · cannot open project".into());
                return;
            }
        };
        self.choose_project_prepared(path, mode, needs_confirmation);
    }
    fn project_replace_confirmation(&self) -> Result<Option<bool>, SnapshotTransportError> {
        let Some(state) = self.state.as_ref() else {
            return Ok(None);
        };
        state.read_with(|snapshot| {
            Some(
                snapshot.model().tracks().len() != 0
                    || snapshot.model().has_assets()
                    || snapshot.can_undo()
                    || snapshot.can_redo()
                    || self.tuning.client.snapshot().is_some(),
            )
        })
    }
    fn choose_project_prepared(
        &mut self,
        path: std::path::PathBuf,
        mode: hex_session::DocumentAudioMode,
        needs_confirmation: bool,
    ) {
        if path.as_os_str().len() > 4_096 {
            self.error = Some("project path exceeds the 4096-byte limit".into());
            return;
        }
        if needs_confirmation {
            if self.pending_open_path.as_ref() == Some(&path)
                && self.pending_open_mode == Some(mode)
            {
                self.begin_open(path, mode);
                return;
            }
            self.pending_open_path = Some(path);
            self.pending_open_mode = Some(mode);
            self.status = "replace project · open again to confirm".into();
            self.error = None;
            return;
        }
        self.begin_open(path, mode);
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
        if self.opening_project {
            return;
        }
        let has_content = match self.state.as_ref() {
            Some(state) => match state.read_with(|snapshot| {
                snapshot.unsaved()
                    || snapshot.model().tracks().len() != 0
                    || snapshot.model().has_assets()
            }) {
                Ok(content) => content,
                Err(_) => {
                    self.error = Some("project read unavailable · cannot replace project".into());
                    return;
                }
            },
            None => false,
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
        let track = if self.mode == super::EditorMode::Arrangement {
            match (self.selected_track, self.state.as_ref()) {
                (Some(track), Some(state)) => match state
                    .read_with(|snapshot| snapshot.model().track(track).map(|_| track))
                {
                    Ok(track) => track,
                    Err(_) => {
                        self.error =
                            Some("project read unavailable · cannot choose import target".into());
                        return;
                    }
                },
                _ => None,
            }
        } else {
            None
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
            let picked = response.await;
            let _ = this.update(cx, |view, cx| {
                match picked {
                    Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                        if let Some(path) = paths.pop() {
                            let target_valid = match view.state.as_ref() {
                                Some(state) => match state.read_with(|snapshot| {
                                    let project = snapshot.model();
                                    view.project_generation == project_generation && project.track(track).is_some() && project.scene(scene).is_some()
                                        && matches!(project.session_cell(track, scene), SnapshotSessionCellContentRead::Empty)
                                }) {
                                    Ok(valid) => valid,
                                    Err(_) => { view.error = Some("project read unavailable · cannot import to session".into()); cx.notify(); return; }
                                },
                                None => false,
                            };
                            if !target_valid {
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
