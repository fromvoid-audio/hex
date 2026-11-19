//! Shared offline project and editor window for rendered geometry checks.
use super::{EditorView, install_close_guard};
use gpui::{AppContext, Context, Empty, TestAppContext, Window};
use hex_project::{Command, Created, Editor, Project};
use hex_session::service::{Service, ViewState};
use std::{path::PathBuf, sync::Arc};

pub(super) const TIMELINE_FRAMES: u64 = 384_000;

pub(super) fn audio_project() -> (
    Project,
    hex_project::TrackId,
    hex_project::TrackId,
    hex_project::ClipId,
) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let Created::Imported {
        track: source,
        clip,
        ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio source".into(),
            path: "missing.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .expect("import")
    else {
        panic!("import result")
    };
    let Created::Track(target) = editor
        .execute(Command::AddTrack {
            name: "target".into(),
        })
        .expect("target track")
    else {
        panic!("track result")
    };
    (editor.project().clone(), source, target, clip)
}

#[cfg(test)]
pub(super) fn editor_window(
    cx: &mut TestAppContext,
    project: Project,
    midi_preview: Arc<hex_engine::MidiArrangementPreview>,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    editor_window_with_timeline(cx, project, midi_preview, TIMELINE_FRAMES)
}

#[cfg(test)]
pub(super) fn editor_window_with_timeline(
    cx: &mut TestAppContext,
    project: Project,
    midi_preview: Arc<hex_engine::MidiArrangementPreview>,
    timeline_frames: u64,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    project_window(
        cx,
        project,
        midi_preview,
        timeline_frames,
        |window, cx, service| EditorView::new(window, cx, service, Err("offline ui test".into())),
    )
}

pub(super) fn recorder_window(
    cx: &mut TestAppContext,
    project: Project,
    root: PathBuf,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    let home = root.join("home");
    let directory = home.join("Music");
    std::fs::create_dir_all(&directory).expect("private recorder browser directory");
    cx.update(|app| app.use_test_assets(crate::assets::HexAssets));
    project_window(
        cx,
        project,
        Default::default(),
        TIMELINE_FRAMES,
        move |window, cx, service| {
            EditorView::new_with_browser_paths(
                window,
                cx,
                service,
                Err("offline ui test".into()),
                (
                    root.join("browser.json"),
                    root.join("recent.json"),
                    root.join("theme.json"),
                    root.join("project-directory.json"),
                ),
                Some(super::sampler_load::FixturePaths { home, directory }),
                #[cfg(test)]
                super::test_environment::Inputs::default(),
            )
        },
    )
}

fn project_window(
    cx: &mut TestAppContext,
    project: Project,
    midi_preview: Arc<hex_engine::MidiArrangementPreview>,
    timeline_frames: u64,
    view: impl FnOnce(&mut Window, &mut Context<EditorView>, Service) -> EditorView,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("disposable test service");
            let editor = window.replace_root(cx, |window, cx| view(window, cx, service));
            install_close_guard(window, cx, editor.downgrade());
            editor.update(cx, |view, cx| {
                // Geometry fixtures use a fixed frame span, independent of the
                // application's default trailing navigation allowance.
                view.viewport = Some(super::viewport::Viewport::fit(timeline_frames));
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: std::collections::BTreeMap::new(),
                    midi_preview,
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames,
                    can_undo: false,
                    can_redo: false,
                    revision: 1,
                    unsaved: false,
                    document_path: None,
                }));
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    (handle, editor)
}
