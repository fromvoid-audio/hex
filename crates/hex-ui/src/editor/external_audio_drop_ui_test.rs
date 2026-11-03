use super::clip_drag_ui_test::{audio_project, editor_window};
use gpui::{AppContext, Modifiers, TestAppContext, VisualTestContext, point};
use std::{path::PathBuf, sync::Arc};

#[gpui::test]
fn external_audio_drop_uses_regular_lane_and_gap_import_contracts(cx: &mut TestAppContext) {
    for gap in [false, true] {
        let (project, source, target, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let source_lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", source.value()).into_boxed_str(),
            ))
            .expect("source lane");
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", target.value()).into_boxed_str(),
            ))
            .expect("empty target lane");
        let pointer = point(
            lane.origin.x + lane.size.width * 0.5,
            if gap {
                source_lane.bottom_left().y
            } else {
                lane.origin.y + lane.size.height * 0.5
            },
        );
        visual.simulate_mouse_move(pointer, None, Modifiers::default());
        let path = PathBuf::from("/Original Case/Loop.WAV");
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.drop_external_audio_paths(std::slice::from_ref(&path), window, cx)
            });
        })
        .expect("native path adapter");
        editor.read_with(cx, |view, _| {
            if gap {
                let (retained, before, _) = view
                    .last_enqueued_import_to_new_track
                    .as_ref()
                    .expect("gap creates track");
                assert_eq!(retained, &path);
                assert_eq!(*before, Some(target));
                assert!(view.last_enqueued_import_to_track.is_none());
            } else {
                let (retained, track, start, overwrite) = view
                    .last_enqueued_import_to_track
                    .as_ref()
                    .expect("existing lane import");
                assert_eq!(retained, &path);
                assert_eq!(*track, target);
                assert!(*start > 0);
                assert!(*overwrite);
                assert!(view.last_enqueued_import_to_new_track.is_none());
            }
        });
    }
}

#[gpui::test]
fn external_audio_drop_refuses_main_stale_geometry_busy_and_empty_paths(cx: &mut TestAppContext) {
    for refusal in 0..4 {
        let (project, _, target, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = visual
            .debug_bounds(if refusal == 0 {
                "main-track-lane"
            } else {
                Box::leak(format!("arrangement-lane-{}", target.value()).into_boxed_str())
            })
            .expect("drop lane");
        let pointer = point(
            lane.origin.x + lane.size.width * 0.5,
            lane.origin.y + lane.size.height * 0.5,
        );
        visual.simulate_mouse_move(pointer, None, Modifiers::default());
        let mut paths = vec![PathBuf::from("/Original Case/Loop.WAV")];
        if refusal == 3 {
            paths.clear();
        }
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                if refusal == 1 {
                    view.drag_layout_generation = view.drag_layout_generation.wrapping_add(1);
                }
                if refusal == 2 {
                    view.opening_project = true;
                }
                view.drop_external_audio_paths(&paths, window, cx);
                assert!(view.last_enqueued_import_to_track.is_none());
                assert!(view.last_enqueued_import_to_new_track.is_none());
                assert!(view.error.is_some());
                assert_eq!(
                    view.state.as_ref().expect("project").project.tracks().len(),
                    2
                );
            });
        })
        .expect("refused drop");
    }
}

#[gpui::test]
fn external_audio_drop_in_empty_or_trailing_space_appends_one_track(cx: &mut TestAppContext) {
    for empty in [false, true] {
        let (project, _, target, _) = audio_project();
        let project = if empty {
            hex_project::Project::new(48_000).expect("empty project")
        } else {
            project
        };
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let body = visual.debug_bounds("arrangement-content").expect("body");
        let y = if empty {
            body.origin.y + body.size.height * 0.5
        } else {
            let last = visual
                .debug_bounds(Box::leak(
                    format!("arrangement-lane-{}", target.value()).into_boxed_str(),
                ))
                .expect("last lane");
            last.bottom_left().y + gpui::px(12.0)
        };
        visual.simulate_mouse_move(
            point(body.origin.x + body.size.width * 0.4, y),
            None,
            Modifiers::default(),
        );
        let path = PathBuf::from("/Original Case/Loop.WAV");
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.drop_external_audio_paths(std::slice::from_ref(&path), window, cx);
                let (retained, before, _) = view
                    .last_enqueued_import_to_new_track
                    .as_ref()
                    .expect("append one new track");
                assert_eq!(retained, &path);
                assert_eq!(*before, None);
                assert!(view.last_enqueued_import_to_track.is_none());
                assert!(view.error.is_none());
            });
        })
        .expect("native empty-space adapter");
    }
}

pub(super) struct BatchFiles(std::path::PathBuf);
impl BatchFiles {
    pub(super) fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        // A reused PID can meet a leftover directory; take the next free name.
        loop {
            let p = std::env::temp_dir().join(format!(
                "hex-batch-ui-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            match std::fs::create_dir(&p) {
                Ok(()) => return Self(p),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create batch fixture directory: {error}"),
            }
        }
    }
    pub(super) fn wav(&self, name: &str, rate: u32, count: u32) -> PathBuf {
        let n = count * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + n).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&n.to_le_bytes());
        for _ in 0..count {
            bytes.extend_from_slice(&8192_i16.to_le_bytes());
        }
        let p = self.0.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }
}
impl Drop for BatchFiles {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn receive_worker(
    editor: &gpui::Entity<super::EditorView>,
    cx: &mut TestAppContext,
    initial: bool,
) {
    let end = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(cx, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            if initial {
                view.state.as_ref().is_some_and(|s| s.revision == 0)
            } else {
                view.worker_requests_pending == 0 && view.audio_batch.is_none()
            }
        });
        if done {
            return;
        }
        assert!(
            std::time::Instant::now() < end,
            "real worker completion timed out"
        );
        std::thread::yield_now();
    }
}

#[gpui::test]
fn external_audio_batch_ui_to_real_service_orders_media_and_whole_undo_redo(
    cx: &mut TestAppContext,
) {
    let files = BatchFiles::new();
    let paths = vec![files.wav("Z.WAV", 24_000, 3), files.wav("a.wav", 48_000, 7)];
    let (handle, editor) = editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    // Replace the layout fixture's model with the actual worker's initial state.
    receive_worker(&editor, cx, true);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let body = visual.debug_bounds("arrangement-content").unwrap();
    visual.simulate_mouse_move(
        point(
            body.origin.x + body.size.width * 0.4,
            body.origin.y + body.size.height * 0.5,
        ),
        None,
        Modifiers::default(),
    );
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            view.drop_external_audio_paths(&paths, window, cx);
            assert!(view.audio_batch.is_some());
            assert!(view.error.is_none());
            assert_eq!(view.state.as_ref().unwrap().project.tracks().len(), 0);
        })
    })
    .unwrap();
    receive_worker(&editor, cx, false);
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.revision, 1);
        assert!(state.unsaved);
        assert_eq!(state.waveforms.len(), 2);
        assert!(view.audio_batch.is_none());
        assert_eq!(state.project.tracks().len(), 2);
        for (i, t) in state.project.tracks().iter().enumerate() {
            let clip = &t.clips()[0];
            let asset = state
                .project
                .assets()
                .find(|a| a.id() == clip.asset())
                .unwrap();
            assert_eq!(asset.path(), paths[i]);
            assert_eq!(clip.duration(), if i == 0 { 6 } else { 7 });
        }
        assert_eq!(
            state.project.tracks()[0].clips()[0].start(),
            state.project.tracks()[1].clips()[0].start()
        );
    });
    for path in paths {
        std::fs::remove_file(path).unwrap();
    }
    editor.update(cx, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Undo))
    });
    receive_worker(&editor, cx, false);
    editor.read_with(cx, |view, _| {
        let s = view.state.as_ref().unwrap();
        assert!(s.project.tracks().is_empty());
        assert!(!s.can_undo);
        assert!(s.can_redo);
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Redo))
    });
    receive_worker(&editor, cx, false);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.tracks().len(), 2)
    });
}

#[gpui::test]
fn external_audio_batch_ui_to_real_service_bad_member_and_stale_context_are_atomic(
    cx: &mut TestAppContext,
) {
    let files = BatchFiles::new();
    let good = files.wav("good.wav", 48_000, 4);
    let bad = files.0.join("bad.mp3");
    std::fs::write(&bad, b"unsupported").unwrap();
    for stale in [false, true] {
        let (handle, editor) = editor_window(
            cx,
            hex_project::Project::new(48_000).unwrap(),
            Arc::default(),
        );
        receive_worker(&editor, cx, true);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let body = visual.debug_bounds("arrangement-content").unwrap();
        visual.simulate_mouse_move(
            point(
                body.origin.x + body.size.width * 0.5,
                body.origin.y + body.size.height * 0.5,
            ),
            None,
            Modifiers::default(),
        );
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                if stale {
                    view.project_generation = 1;
                }
                view.drop_external_audio_paths(&[good.clone(), bad.clone()], window, cx);
                assert!(view.audio_batch.is_some());
            })
        })
        .unwrap();
        receive_worker(&editor, cx, false);
        editor.read_with(cx, |view, _| {
            let s = view.state.as_ref().unwrap();
            assert_eq!(s.revision, 0);
            assert!(s.project.tracks().is_empty());
            assert!(s.waveforms.is_empty());
            assert!(!s.unsaved);
            assert!(!s.can_undo);
            assert!(view.error.is_some());
            assert!(view.audio_batch.is_none());
        });
    }
}

#[gpui::test]
fn external_audio_cancel_button_reaches_real_worker_without_fifo_cancel_command(
    cx: &mut TestAppContext,
) {
    let files = BatchFiles::new();
    let paths = vec![
        files.wav("first.wav", 48_000, 4),
        files.wav("second.wav", 48_000, 4),
    ];
    let (handle, editor) = editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    receive_worker(&editor, cx, true);
    let (request, control) = editor.read_with(cx, |view, _| {
        hex_session::service::Request::audio_batch(
            paths,
            hex_session::import_batch::AudioBatchDestination::NewTracks {
                before_track: None,
                start: 0,
            },
            hex_session::import_batch_decode::AudioBatchContext {
                project_generation: view.project_generation,
                revision: view.state.as_ref().unwrap().revision,
                active_gesture: false,
            },
        )
        .unwrap()
    });
    // Render actual queued metadata before submission, so cancellation wins
    // deterministically rather than depending on filesystem/thread speed.
    editor.update(cx, |view, cx| {
        view.audio_batch = Some(Arc::clone(&control));
        view.poll_audio_batch();
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let button = visual
        .debug_bounds("cancel-audio-import")
        .expect("rendered cancel target");
    visual.simulate_click(button.center(), Modifiers::default());
    assert!(control.progress().cancellation_requested);
    editor.update(cx, |view, _| assert!(view.enqueue(request)));
    receive_worker(&editor, cx, false);
    editor.read_with(cx, |view, _| {
        let s = view.state.as_ref().unwrap();
        assert_eq!(s.revision, 0);
        assert!(s.project.tracks().is_empty());
        assert!(s.waveforms.is_empty());
        assert!(!s.unsaved);
        assert!(!s.can_undo);
        assert!(view.audio_batch.is_none());
        assert!(view.error.as_ref().unwrap().contains("canceled"));
    });
    assert_eq!(
        control.progress().phase,
        hex_session::import_batch_decode::AudioBatchPhase::Canceled
    );
}
