//! Console drop regressions use real events, the real worker and retained private roots.
use super::{BrowserSection, EditorMode, EditorView};
use gpui::{
    AppContext, Empty, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_library::{DirectoryListing, EntryKind, LibraryEntry};
use hex_project::{Command, SceneId, TrackId};
use hex_session::service::{Request, Service};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

fn receive_worker(editor: &gpui::Entity<EditorView>, cx: &mut TestAppContext) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let done = editor.update(cx, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.state.is_some() && view.worker_requests_pending == 0 && view.audio_batch.is_none()
        });
        if done {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "real Console worker timed out"
        );
        std::thread::yield_now();
    }
}

fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    TrackId,
    SceneId,
    PathBuf,
) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    cx.update(super::super::bind_editor_keys);
    let handle = *cx.add_window(|_, _| Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                // Since #414 the ordinary test constructor allocates and retains all
                // three preference roots and private home/Music BEFORE startup jobs.
                EditorView::new(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Err("offline test".into()),
                )
            })
        })
        .unwrap();
    receive_worker(&editor, cx);
    for request in [
        Request::Edit(Command::AddTrack {
            name: "target".into(),
        }),
        Request::Edit(Command::AddScene {
            name: "first".into(),
        }),
    ] {
        editor.update(cx, |view, _| assert!(view.enqueue(request)));
        receive_worker(&editor, cx);
    }
    let (track, scene, path) = editor.update(cx, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let track = state.project.tracks()[0].id();
        let scene = state.project.scenes()[0].id();
        let directory = view.test_environment.home.join("Music");
        assert!(directory.starts_with(view.test_environment.root()));
        let path = directory.join(format!(
            "ConsoleCase-{}.WAV",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let frames = 512_u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&96_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(frames * 2).to_le_bytes());
        for _ in 0..frames {
            bytes.extend_from_slice(&8192_i16.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        view.browser.scans.generation = view.browser.scans.generation.saturating_add(1);
        view.browser.places = vec![directory.clone()];
        view.browser.directory = directory.clone();
        view.browser.loading = false;
        view.browser.error = None;
        view.browser.listing = Some(DirectoryListing {
            directory: directory.clone(),
            entries: vec![LibraryEntry {
                name: "ConsoleCase.WAV".into(),
                path: path.clone(),
                kind: EntryKind::WaveAudio,
            }],
            enumerated_entries: 1,
            truncated: false,
        });
        view.browser.section = BrowserSection::Place(directory);
        view.mode = EditorMode::Session;
        view.selected_cell = Some((track, scene));
        cx.notify();
        (track, scene, path)
    });
    (handle, editor, track, scene, path)
}

#[gpui::test]
fn console_browser_drop_real_empty_cell_retires_ghost_and_imports_audio(cx: &mut TestAppContext) {
    let (handle, editor, track, scene, path) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("actual browser audio row");
    let column = visual
        .debug_bounds(Box::leak(
            format!("session-track-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let row = visual.debug_bounds("session-scene-column").unwrap();
    let target = point(column.center().x, row.center().y);
    visual.simulate_mouse_down(source.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(source.center().x + px(14.0), source.center().y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            view.active_browser_audio_drag.is_some(),
            "actual browser drag began"
        )
    });
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(
            view.active_browser_audio_drag.is_none(),
            "Console release must retire the browser ghost"
        )
    });
    receive_worker(&editor, cx);
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        let Some(hex_project::SessionCellContent::Audio(clip)) =
            state.project.session_cell(track, scene)
        else {
            panic!("drop did not create a canonical audio clip");
        };
        assert_eq!(state.project.asset(clip.asset()).unwrap().path(), path);
        assert_eq!(clip.source_end(), 512);
        assert!(state.waveforms.contains_key(&clip.asset()));
    });
}
