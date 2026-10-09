use super::EditorView;
use gpui::{Context, PathPromptOptions};
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

impl EditorView {
    pub(super) fn open_dialog(&mut self, cx: &mut Context<Self>) {
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        if self.opening_project {
            return;
        }
        if let Some(path) = self.pending_open_path.take() {
            self.begin_open(path);
            cx.notify();
            return;
        }
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("open project".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = response.await;
            let _ = this.update(cx, |view, cx| {
                match picked {
                    Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                        if let Some(path) = paths.pop() {
                            if super::device_panel::finalize_controls(view, cx) {
                                view.choose_project(path);
                            }
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

    pub(super) fn choose_project(&mut self, path: std::path::PathBuf) {
        if path.as_os_str().len() > 4_096 {
            self.error = Some("project path exceeds the 4096-byte limit".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            self.error = Some("project is still loading".into());
            return;
        };
        let project = &state.project;
        let has_content = !project.tracks().is_empty() || project.assets().next().is_some();
        if has_content || state.can_undo || state.can_redo {
            if self.pending_open_path.as_ref() == Some(&path) {
                self.begin_open(path);
                return;
            }
            self.pending_open_path = Some(path);
            self.status = "replace project · open again to confirm".into();
            self.error = None;
            return;
        }
        self.begin_open(path);
    }

    fn begin_open(&mut self, path: std::path::PathBuf) {
        self.pending_open_path = None;
        self.cancel_duplicate_chain();
        if self.enqueue(Request::Open(path)) {
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
                    Ok(Err(error)) => view.error = Some(error.to_string().to_lowercase()),
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
                    Ok(Err(error)) => view.error = Some(error.to_string().to_lowercase()),
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
