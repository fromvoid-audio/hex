//! Background persistence and native menu routing for recent project paths.
use super::{EditorView, OpenRecentProject, menus};
use gpui::Context;
use hex_preferences::RecentProjects;
use std::path::{Path, PathBuf};

pub(super) struct RecentProjectsState {
    path: PathBuf,
    pub(super) projects: RecentProjects,
    loading: bool,
    writable: bool,
    changed_while_loading: bool,
    save_in_flight: bool,
    pending_save: Option<RecentProjects>,
}

impl RecentProjectsState {
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn test_path(&self) -> &Path {
        &self.path
    }
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            projects: RecentProjects::default(),
            loading: true,
            writable: true,
            changed_while_loading: false,
            save_in_flight: false,
            pending_save: None,
        }
    }
}

#[cfg(not(test))]
pub(super) fn default_preferences_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    hex_preferences::recent_projects_path(&home)
}

impl EditorView {
    pub(super) fn start_recent_projects_load(&mut self, cx: &mut Context<Self>) {
        let path = self.recent_projects.path.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::load_recent_projects(&path)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.recent_projects_loaded(result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn recent_projects_loaded(
        &mut self,
        result: Result<RecentProjects, hex_preferences::PreferencesError>,
        cx: &mut Context<Self>,
    ) {
        self.recent_projects.loading = false;
        match result {
            Err(error) => {
                self.recent_projects.writable = false;
                self.error = Some(
                    format!(
                        "could not load recent projects · {}",
                        error.to_string().to_lowercase()
                    )
                    .into(),
                );
            }
            Ok(loaded) => {
                if self.recent_projects.changed_while_loading {
                    let mut merged = loaded;
                    for path in self.recent_projects.projects.paths().iter().rev() {
                        if let Err(error) = merged.record_opened(path.clone()) {
                            self.recent_projects.writable = false;
                            self.error = Some(error.to_string().to_lowercase().into());
                            break;
                        }
                    }
                    self.recent_projects.projects = merged;
                } else {
                    self.recent_projects.projects = loaded;
                }
                if self.recent_projects.changed_while_loading {
                    self.queue_recent_projects_save(cx);
                }
            }
        }
        menus::install(cx, self.recent_projects.projects.paths());
    }

    pub(super) fn record_opened_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match self.recent_projects.projects.record_opened(path) {
            Ok(false) => return,
            Ok(true) => {}
            Err(error) => {
                self.error = Some(
                    format!(
                        "could not add recent project · {}",
                        error.to_string().to_lowercase()
                    )
                    .into(),
                );
                return;
            }
        }
        if self.recent_projects.loading {
            self.recent_projects.changed_while_loading = true;
        } else {
            self.queue_recent_projects_save(cx);
        }
        menus::install(cx, self.recent_projects.projects.paths());
    }

    fn queue_recent_projects_save(&mut self, cx: &mut Context<Self>) {
        if !self.recent_projects.writable {
            return;
        }
        self.recent_projects.pending_save = Some(self.recent_projects.projects.clone());
        self.start_recent_projects_save(cx);
    }

    fn start_recent_projects_save(&mut self, cx: &mut Context<Self>) {
        if self.recent_projects.loading || self.recent_projects.save_in_flight {
            return;
        }
        let Some(projects) = self.recent_projects.pending_save.take() else {
            return;
        };
        self.recent_projects.save_in_flight = true;
        let path = self.recent_projects.path.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::save_recent_projects(&path, &projects)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.recent_projects_saved(result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn recent_projects_saved(
        &mut self,
        result: Result<(), hex_preferences::PreferencesError>,
        cx: &mut Context<Self>,
    ) {
        self.recent_projects.save_in_flight = false;
        if self.recent_projects.pending_save.is_some() {
            self.start_recent_projects_save(cx);
        } else if let Err(error) = result {
            self.error = Some(
                format!(
                    "could not save recent projects · {}",
                    error.to_string().to_lowercase()
                )
                .into(),
            );
        }
    }

    pub(super) fn open_recent_project(&mut self, action: &OpenRecentProject) {
        if self.opening_project {
            return;
        }
        if !self.recent_projects.projects.paths().contains(&action.path) {
            self.error = Some("recent project is no longer in the menu".into());
            return;
        }
        self.choose_project(
            action.path.clone(),
            hex_session::DocumentAudioMode::Resident,
        );
    }
}

pub(super) fn recent_project_label(path: &Path, all_paths: &[PathBuf]) -> String {
    let file_name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .to_lowercase();
    let duplicate_name = all_paths
        .iter()
        .filter(|other| {
            other
                .file_name()
                .unwrap_or(other.as_os_str())
                .to_string_lossy()
                .eq_ignore_ascii_case(&file_name)
        })
        .count()
        > 1;
    if duplicate_name {
        let parent = path
            .parent()
            .unwrap_or(Path::new(""))
            .to_string_lossy()
            .to_lowercase();
        format!("{file_name} · {parent}")
    } else {
        file_name
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{AppContext, Empty, TestAppContext};
    use hex_project::Project;
    use hex_session::service::{Service, ViewState};
    use std::sync::Arc;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "hex-ui-recent-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("create isolated preferences directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn duplicate_filename_labels_include_lowercase_parent_path() {
        let paths = [
            PathBuf::from("/Work/Live/Session.HEX"),
            PathBuf::from("/Archive/Session.HEX"),
        ];
        assert_eq!(
            recent_project_label(&paths[0], &paths),
            "session.hex · /work/live"
        );
    }

    fn editor_fixture(
        cx: &mut TestAppContext,
        directory: &TestDirectory,
    ) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
        let bootstrap = cx.add_window(|_, _| Empty);
        let handle = *bootstrap;
        let recent_path = directory.0.join("recent-projects.json");
        let browser_path = directory.0.join("browser.json");
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let service = Service::new(48_000, 32, 8_000_000).expect("test service");
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new_for_test(
                        window,
                        cx,
                        service,
                        Err("offline test".into()),
                        browser_path,
                        recent_path,
                    )
                });
                editor.update(cx, |view, _| {
                    view.state = Some(Arc::new(ViewState {
                        canonical_audio_source: None,
                        comparison_audio: None,
                        project: hex_session::SharedProject::new_on_control(
                            Project::new(48_000).expect("project"),
                        ),
                        waveforms: Default::default(),
                        audio_clip_waveforms: Default::default(),
                        midi_preview: hex_engine::empty_midi_preview_on_control(),
                        gain_preview: Ok(hex_engine::SharedGainAutomationPreview::new_on_control(
                            hex_engine::GainAutomationPreview::default(),
                        )),
                        timeline_frames: 384_000,
                        can_undo: false,
                        can_redo: false,
                        revision: 0,
                        unsaved: false,
                        document_path: None,
                    }));
                });
                editor
            })
            .expect("editor window");
        (handle, editor)
    }

    #[gpui::test]
    fn recent_menu_action_keeps_its_path_after_mru_reordering_and_obeys_open_guard(
        cx: &mut TestAppContext,
    ) {
        let directory = TestDirectory::new();
        let (_handle, editor) = editor_fixture(cx, &directory);
        let first = directory.0.join("first.hex");
        let second = directory.0.join("second.hex");
        editor.update(cx, |view, _| {
            view.recent_projects.projects =
                RecentProjects::new([first.clone(), second.clone()]).expect("recent paths");
        });
        let stale_menu_action = OpenRecentProject {
            path: first.clone(),
        };
        editor.update(cx, |view, _| {
            view.recent_projects
                .projects
                .record_opened(second.clone())
                .expect("reorder second path");
            let current = view.state.as_ref().expect("state");
            let dirty = ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project: current.project.clone(),
                waveforms: Default::default(),
                audio_clip_waveforms: Default::default(),
                midi_preview: current.midi_preview.clone(),
                gain_preview: Ok(hex_engine::SharedGainAutomationPreview::new_on_control(
                    hex_engine::GainAutomationPreview::default(),
                )),
                timeline_frames: current.timeline_frames,
                can_undo: true,
                can_redo: false,
                revision: current.revision,
                unsaved: true,
                document_path: current.document_path.clone(),
            };
            view.state = Some(Arc::new(dirty));
            view.open_recent_project(&stale_menu_action);
            assert_eq!(view.pending_open_path.as_ref(), Some(&first));
            assert!(!view.opening_project);
            view.open_recent_project(&stale_menu_action);
            assert!(
                !view.opening_project,
                "another click must not bypass confirmation"
            );
            assert_eq!(view.pending_open_path.as_ref(), Some(&first));
            assert!(view.pending_requests.is_empty());
            let pending_count = view.pending_requests.len();
            view.opening_project = true;
            view.open_recent_project(&OpenRecentProject {
                path: second.clone(),
            });
            assert_eq!(view.pending_requests.len(), pending_count);
        });
    }

    #[gpui::test]
    fn open_during_recent_load_merges_a_new_path_ahead_of_full_saved_mru(cx: &mut TestAppContext) {
        use gpui::VisualTestContext;
        let directory = TestDirectory::new();
        let recent_path = directory.0.join("recent-projects.json");
        let persisted_paths = (0..hex_preferences::MAX_RECENT_PROJECTS)
            .map(|index| directory.0.join(format!("saved-{index}.hex")))
            .collect::<Vec<_>>();
        let persisted = RecentProjects::new(persisted_paths.clone()).expect("full saved MRU");
        hex_preferences::save_recent_projects(&recent_path, &persisted)
            .expect("write isolated full MRU");
        let (handle, editor) = editor_fixture(cx, &directory);
        let opened = directory.0.join("opened-before-load.hex");
        editor.update(cx, |view, cx| {
            view.recent_projects.loading = true;
            view.record_opened_project(opened.clone(), cx);
            view.recent_projects_loaded(Ok(persisted.clone()), cx);
            assert!(view.recent_projects.writable);
            assert_eq!(view.recent_projects.projects.paths().len(), 10);
            assert_eq!(view.recent_projects.projects.paths()[0], opened);
            assert!(
                !view
                    .recent_projects
                    .projects
                    .paths()
                    .contains(&persisted_paths[9])
            );
        });
        let visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        assert_eq!(
            hex_preferences::load_recent_projects(&recent_path).expect("saved merged MRU"),
            editor.read_with(cx, |view, _| view.recent_projects.projects.clone())
        );
    }
}
