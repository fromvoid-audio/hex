//! Real Service open of a saved project whose media moved; disposable files only.
use super::*;
use crate::editor::test_environment;
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};
use hex_session::service::Service;
use std::time::Instant;

fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.state.is_some()
                && view.pending_open_path.is_none()
        });
        visual.run_until_parked();
        if done {
            return;
        }
        assert!(Instant::now() < until, "real Service did not settle");
        std::thread::yield_now();
    }
}

fn wav(path: &std::path::Path, frames: u32) {
    let mut bytes = Vec::from(*b"RIFF");
    bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(frames * 2).to_le_bytes());
    bytes.resize(bytes.len() + frames as usize * 2, 0);
    std::fs::write(path, bytes).unwrap();
}

#[gpui::test]
fn moved_media_shows_panel_instead_of_failing_and_opens_with_relink(cx: &mut TestAppContext) {
    let environment = test_environment::Environment::new().unwrap();
    let root = environment.root().to_path_buf();
    std::fs::create_dir(root.join("samples")).unwrap();
    std::fs::create_dir(root.join("moved")).unwrap();
    let original = root.join("samples/kick.wav");
    wav(&original, 64);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    pump(&editor, &mut visual);
    let song = root.join("song.hex");
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Import(original.clone())));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveNew(song.clone())));
    });
    pump(&editor, &mut visual);
    let moved = root.join("moved/kick.wav");
    std::fs::rename(&original, &moved).unwrap();
    let generation = editor.read_with(&visual, |view, _| view.project_generation);

    editor.update(&mut visual, |view, cx| {
        view.choose_project(song.clone(), DocumentAudioMode::Resident);
        cx.notify();
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.opening_project);
        assert_eq!(view.project_generation, generation);
        let panel = view.missing_media.as_ref().expect("missing-media panel");
        assert_eq!(panel.rows.len(), 1);
        assert_eq!(panel.rows[0].media.expected_path, original);
        assert_eq!(panel.rows[0].media.usage_count, 1);
    });
    assert!(visual.debug_bounds("missing-media-overlay").is_some());

    // The open stays disabled until every file is resolved.
    editor.update(&mut visual, |view, cx| view.missing_media_open(cx));
    assert!(editor.read_with(&visual, |view, _| view.missing_media.is_some()));
    editor.update(&mut visual, |view, cx| {
        let panel = view.missing_media.as_mut().unwrap();
        let media = panel.rows[0].media.clone();
        let fit = missing_media::check_candidate(&media, &moved);
        apply_locate(&mut panel.rows, 0, moved.clone(), fit, Vec::new());
        view.missing_media_open(cx);
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.missing_media.is_none() && !view.opening_project);
        assert_eq!(view.project_generation, generation + 1);
        let state = view.state.as_ref().unwrap();
        assert!(state.unsaved);
        assert_eq!(state.project.assets().next().unwrap().path(), moved);
    });
    assert!(visual.debug_bounds("missing-media-overlay").is_none());
}
