//! The truncated-WAV recovery prompt is offered only for a truncation refusal,
//! cancelling imports nothing and confirming recovers the readable frames.
use super::super::external_audio_drop_ui_test::BatchFiles;
use super::super::{EditorView, clip_drag_ui_test::editor_window};
use gpui::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use hex_session::service::Request;
use std::sync::Arc;

/// Drive the real worker until every submitted request has its terminal.
fn settle(cx: &mut TestAppContext, handle: AnyWindowHandle, editor: &Entity<EditorView>) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        // Park first so a just-answered prompt's task has enqueued its request.
        cx.run_until_parked();
        let ready = editor.update(cx, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.state.is_some() && !view.edit_pending()
        });
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.reconcile_truncated_import_offer(window, cx)
            })
        })
        .unwrap();
        cx.run_until_parked();
        if ready {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "real worker settles");
        std::thread::yield_now();
    }
}

fn import(
    cx: &mut TestAppContext,
    handle: AnyWindowHandle,
    editor: &Entity<EditorView>,
    path: &std::path::Path,
) {
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Import(path.to_path_buf())))
    });
    settle(cx, handle, editor);
}

fn asset_frames(cx: &TestAppContext, editor: &Entity<EditorView>) -> Vec<u64> {
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().expect("state").project;
        project
            .assets()
            .map(|asset| asset.source_frames())
            .collect()
    })
}

#[gpui::test]
fn recovery_prompt_appears_only_for_truncation_and_cancel_imports_nothing(cx: &mut TestAppContext) {
    let files = BatchFiles::new();
    let intact = files.wav("intact.wav", 48_000, 64);
    let cut = files.wav("cut.wav", 48_000, 128);
    let mut bytes = std::fs::read(&cut).unwrap();
    // 96 whole frames remain, plus one byte of a partial frame.
    bytes.truncate(bytes.len() - 32 * 2 + 1);
    std::fs::write(&cut, &bytes).unwrap();
    let garbage = intact.with_file_name("garbage.wav");
    std::fs::write(&garbage, b"not a wav file").unwrap();

    let (handle, editor) = editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    settle(cx, handle, &editor);

    import(cx, handle, &editor, &intact);
    assert!(!cx.has_pending_prompt(), "an intact import offers nothing");
    assert_eq!(asset_frames(cx, &editor), [64]);

    import(cx, handle, &editor, &garbage);
    assert!(
        editor.read_with(cx, |view, _| view.error.is_some()),
        "malformed import is refused"
    );
    assert!(!cx.has_pending_prompt(), "other refusals offer no recovery");

    import(cx, handle, &editor, &cut);
    assert!(
        cx.has_pending_prompt(),
        "truncation refusal offers recovery"
    );
    assert_eq!(asset_frames(cx, &editor), [64], "refusal imports nothing");
    cx.simulate_prompt_answer("cancel");
    settle(cx, handle, &editor);
    assert!(!cx.has_pending_prompt());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.status, "truncated recording not imported");
        assert!(view.pending_requests.is_empty());
        assert_eq!(view.worker_requests_pending, 0, "cancel submits nothing");
    });
    assert_eq!(asset_frames(cx, &editor), [64], "cancel imports nothing");
    assert_eq!(std::fs::read(&cut).unwrap(), bytes, "original untouched");
}

#[gpui::test]
fn confirming_the_prompt_recovers_exactly_the_readable_frames(cx: &mut TestAppContext) {
    let files = BatchFiles::new();
    let cut = files.wav("cut.wav", 48_000, 128);
    let mut bytes = std::fs::read(&cut).unwrap();
    bytes.truncate(bytes.len() - 32 * 2 + 1);
    std::fs::write(&cut, &bytes).unwrap();

    let (handle, editor) = editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    settle(cx, handle, &editor);
    import(cx, handle, &editor, &cut);
    assert!(
        cx.has_pending_prompt(),
        "truncation refusal offers recovery"
    );
    cx.simulate_prompt_answer("recover readable frames");
    settle(cx, handle, &editor);
    assert!(!cx.has_pending_prompt(), "a recovery is not offered again");
    assert_eq!(asset_frames(cx, &editor), [96]);
    editor.read_with(cx, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.tracks()[0].clips()[0].duration(), 96);
        assert!((project.tracks()[0].gain() - 0.251_188_64).abs() < 1e-7);
    });
    assert_eq!(std::fs::read(&cut).unwrap(), bytes, "original untouched");
}

#[test]
fn recovery_intent_retains_the_exact_captured_gain_and_destination() {
    let path = std::path::PathBuf::from("/mixed Case/source.wav");
    let request = Request::ImportToNewTrackWithGain {
        path: path.clone(),
        before_track: None,
        start: 37,
        gain: 0.25,
    };
    let intent = super::ImportIntent::capture(&request).expect("single-file intent");
    let recovered = intent.into_request();
    assert!(
        matches!(&recovered, Request::ImportToNewTrackWithGain { path: actual, before_track: None, start: 37, gain } if actual == &path && *gain == 0.25)
    );
    assert!(hex_session::service::TruncatedImportRecovery::new(recovered).is_ok());
}

#[gpui::test]
fn single_file_imports_use_captured_default_in_real_worker(cx: &mut TestAppContext) {
    let files = BatchFiles::new();
    let first = files.wav("first.wav", 48_000, 64);
    let second = files.wav("second.wav", 48_000, 64);
    let (handle, editor) = editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    settle(cx, handle, &editor);
    import(cx, handle, &editor, &first);
    let anchor = editor.read_with(cx, |view, _| {
        let track = &view.state.as_ref().unwrap().project.tracks()[0];
        assert!((track.gain() - 0.251_188_64).abs() < 1e-7);
        track.id()
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::ImportToNewTrack {
            path: second,
            before_track: Some(anchor),
            start: 100,
        }))
    });
    settle(cx, handle, &editor);
    editor.read_with(cx, |view, _| {
        let tracks = view.state.as_ref().unwrap().project.tracks();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[1].id(), anchor);
        assert!((tracks[0].gain() - 0.251_188_64).abs() < 1e-7);
        assert_eq!(tracks[0].clips()[0].start(), 100);
    });
}
