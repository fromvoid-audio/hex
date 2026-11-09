use super::clip_drag_ui_test::{audio_project, editor_window, midi_project};
use gpui::{AppContext, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, Command};
use std::sync::Arc;

#[gpui::test]
fn arrangement_nudge_uses_selected_grid_instead_of_legacy_free_step(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    for midi in [false, true] {
        let (project, track, _, clip) = if midi {
            midi_project()
        } else {
            audio_project()
        };
        let (handle, editor) = editor_window(cx, project, Arc::default());
        editor.update(cx, |view, cx| {
            view.selected_track = Some(track);
            view.selected_clip = Some(clip);
            view.snap_grid = super::musical_time::SnapGrid::Sixteenth;
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("right");
        editor.read_with(cx, |view, _| {
            if midi {
                assert!(matches!(view.last_enqueued_edit,
                    Some(Command::MoveMidiClip { start, .. }) if start == BeatTime::from_raw(BeatTime::ONE.raw()*5/4)));
            } else {
                assert!(matches!(view.last_enqueued_edit, Some(Command::MoveClip { start: 6000, .. })), "request={:?} bounds={:?} epoch={} layout={} pending={} mode={:?}", view.last_enqueued_edit, view.arrangement_bounds, view.arrangement_bounds_epoch, view.drag_layout_generation, view.edit_pending(), view.mode);
            }
        });
    }
}

#[gpui::test]
fn arrangement_nudge_free_modifier_and_busy_refusal_preserve_legacy_commands(
    cx: &mut TestAppContext,
) {
    for (grid, platform, busy) in [
        (super::musical_time::SnapGrid::Off, false, false),
        (super::musical_time::SnapGrid::Sixteenth, true, false),
        (super::musical_time::SnapGrid::Sixteenth, false, true),
    ] {
        let (project, track, _, clip) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        editor.update(cx, |view, _| {
            view.selected_track = Some(track);
            view.selected_clip = Some(clip);
            view.snap_grid = grid;
            if busy {
                view.worker_requests_pending = 1;
            }
            view.nudge_audio_clip(clip, true, platform);
            if busy {
                assert!(view.last_enqueued_edit.is_none());
            } else {
                assert!(matches!(
                    view.last_enqueued_edit,
                    Some(Command::NudgeClip { frames: 12000, .. })
                ));
            }
        });
    }
}

#[gpui::test]
fn snap_nudge_worker_history_keeps_source_and_other_tracks(cx: &mut TestAppContext) {
    use hex_session::service::{Request, Service};
    fn next(service: &Service) -> hex_session::service::Event {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            if let Ok(event) = service.try_event() {
                return event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "next: hang guard expired"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    fn pump(editor: &gpui::Entity<super::EditorView>, cx: &mut TestAppContext, revision: u64) {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            editor.update(cx, |view, cx| {
                view.tick(cx);
                cx.notify();
            });
            if editor.read_with(cx, |view, _| {
                view.worker_requests_pending == 0
                    && view
                        .state
                        .as_ref()
                        .is_some_and(|state| state.revision == revision)
            }) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "pump: hang guard expired"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    cx.update(super::bind_editor_keys);
    for midi in [false, true] {
        let service = Service::new(48_000, 16, 16).unwrap();
        next(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "source".into(),
            }))
            .unwrap();
        let track = next(&service).state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "other".into(),
            }))
            .unwrap();
        next(&service);
        if midi {
            service
                .submit(Request::Edit(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ONE,
                    length: BeatTime::from_quarters(4).unwrap(),
                }))
                .unwrap();
        } else {
            let path =
                std::env::temp_dir().join(format!("hex-snap-nudge-{}.wav", std::process::id()));
            let mut wav = Vec::new();
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&44u32.to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16u32.to_le_bytes());
            wav.extend_from_slice(&1u16.to_le_bytes());
            wav.extend_from_slice(&1u16.to_le_bytes());
            wav.extend_from_slice(&48000u32.to_le_bytes());
            wav.extend_from_slice(&96000u32.to_le_bytes());
            wav.extend_from_slice(&2u16.to_le_bytes());
            wav.extend_from_slice(&16u16.to_le_bytes());
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&8u32.to_le_bytes());
            for sample in [8000i16, -8000, 4000, -4000] {
                wav.extend_from_slice(&sample.to_le_bytes());
            }
            std::fs::write(&path, wav).unwrap();
            service
                .submit(Request::ImportToTrack {
                    path,
                    track,
                    start: 3000,
                })
                .unwrap();
        }
        let baseline = next(&service).state;
        let original = baseline.project.clone();
        let revision = baseline.revision;
        let clip = if midi {
            original.tracks()[0].midi_clips()[0].id()
        } else {
            original.tracks()[0].clips()[0].id()
        };
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    super::EditorView::new(window, cx, service, Err("offline ui test".into()))
                });
                editor.update(cx, |view, cx| {
                    view.state = Some(baseline);
                    view.selected_track = Some(track);
                    view.selected_clip = Some(clip);
                    view.snap_grid = super::musical_time::SnapGrid::Sixteenth;
                    cx.notify();
                });
                editor
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("right");
        pump(&editor, cx, revision + 1);
        let moved = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        if midi {
            assert_eq!(
                moved.midi_clip(clip).unwrap().start(),
                BeatTime::from_raw(BeatTime::ONE.raw() * 5 / 4)
            );
            assert_eq!(
                moved.midi_clip(clip).unwrap().source(),
                original.midi_clip(clip).unwrap().source()
            );
            assert_eq!(
                moved.midi_clip(clip).unwrap().length(),
                original.midi_clip(clip).unwrap().length()
            );
        } else {
            let before = original.clip(clip).unwrap();
            let after = moved.clip(clip).unwrap();
            assert_eq!(after.start(), 6000);
            assert_eq!(after.duration(), before.duration());
            assert_eq!(after.asset(), before.asset());
            assert_eq!(after.source_offset(), before.source_offset());
            assert_eq!(after.source_length(), before.source_length());
        }
        assert_eq!(moved.tracks()[1], original.tracks()[1]);
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Undo));
        });
        pump(&editor, cx, revision + 2);
        assert_eq!(
            editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone()),
            original
        );
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Redo));
        });
        pump(&editor, cx, revision + 3);
        assert_eq!(
            editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone()),
            moved
        );
    }
}

#[gpui::test]
fn arranger_toolbar_only_toggles_adaptive_grid(cx: &mut TestAppContext) {
    use super::musical_time::SnapGrid;
    let (project, _, _, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |view, _| view.snap_grid),
        SnapGrid::Adaptive
    );
    for expected in [
        SnapGrid::Off,
        SnapGrid::Adaptive,
        SnapGrid::Off,
        SnapGrid::Adaptive,
    ] {
        let button = visual
            .debug_bounds("arranger-snap-toggle")
            .expect("snap toggle");
        visual.simulate_click(button.center(), gpui::Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.snap_grid, expected);
            assert_eq!(
                view.snap_grid.with_platform_modifier(true),
                expected.toggled()
            );
        });
    }
}
