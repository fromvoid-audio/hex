//! Actual unsupported Shift input must clamp to a prepared-capable boundary.
use super::super::{EditorView, musical_time::SnapGrid};
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_project::{ClipId, Command, TrimEdge};
use hex_session::service::{Event, Request, Service};
use std::time::{Duration, Instant};

fn next(service: &Service) -> Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(
            std::time::Instant::now() < until,
            "next: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
            v.worker_requests_pending == 0 && v.pending_requests.is_empty()
        });
        if done {
            visual.run_until_parked();
            return;
        }
        assert!(
            Instant::now() < until,
            "worker pending at deadline: {:?}",
            editor.read_with(&*visual, |v, _| (
                v.worker_requests_pending,
                v.pending_requests.len(),
                v.state.as_ref().map(|s| s.revision),
                v.error.clone()
            ))
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn fixture(
    cx: &mut TestAppContext,
) -> (
    super::super::external_audio_drop_ui_test::BatchFiles,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    ClipId,
) {
    fixture_at_rate(cx, 48_000)
}
fn fixture_at_rate(
    cx: &mut TestAppContext,
    sample_rate: u32,
) -> (
    super::super::external_audio_drop_ui_test::BatchFiles,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    ClipId,
) {
    fixture_with_source_frames(cx, sample_rate, sample_rate * 2)
}
fn fixture_with_source_frames(
    cx: &mut TestAppContext,
    sample_rate: u32,
    source_frames: u32,
) -> (
    super::super::external_audio_drop_ui_test::BatchFiles,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    ClipId,
) {
    cx.update(super::super::bind_editor_keys);
    let files = super::super::external_audio_drop_ui_test::BatchFiles::new();
    let wav = files.wav("stretch-capability.wav", sample_rate, source_frames);
    let directory = wav.parent().unwrap();
    std::fs::create_dir(directory.join("samples")).unwrap();
    let service = Service::new(sample_rate, 32, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(wav.clone())).unwrap();
    let event = next(&service);
    let clip = event.state.project.tracks()[0].clips()[0].id();
    let track = event.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::MoveClip {
            clip,
            track,
            start: u64::from(source_frames),
        }))
        .unwrap();
    let event = next(&service);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, directory)
            });
            editor.update(cx, |v, cx| {
                assert_eq!(v.browser.directory, directory.join("samples"));
                assert_eq!(v.browser.places, vec![directory.join("samples")]);
                v.state = Some(event.state);
                v.snap_grid = SnapGrid::default().with_platform_modifier(true);
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (files, handle, editor, clip)
}
#[gpui::test]
fn shift_right_preview_boundary_is_accepted_by_actual_worker(cx: &mut TestAppContext) {
    check_boundary(cx, TrimEdge::Right);
}
#[gpui::test]
fn shift_left_preview_boundary_is_accepted_by_actual_worker(cx: &mut TestAppContext) {
    check_boundary(cx, TrimEdge::Left);
}
fn check_boundary(cx: &mut TestAppContext, edge: TrimEdge) {
    let (_files, handle, editor, clip) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let name = if edge == TrimEdge::Right {
        "right"
    } else {
        "left"
    };
    let selector = Box::leak(format!("clip-trim-{name}-{}", clip.value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_mouse_down(down, MouseButton::Left, shift);
    visual.simulate_mouse_move(
        point(
            down.x + px(if edge == TrimEdge::Right { -12.0 } else { 12.0 }),
            down.y,
        ),
        MouseButton::Left,
        shift,
    );
    visual.run_until_parked();
    // Ask for a 1/16 transform, deliberately outside existing 1/8 admission.
    let target = editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        let c = state.project.clip(clip).unwrap();
        let track = state.project.tracks()[0].id();
        let lane = v.drag_lanes[&track];
        let viewport = v.visible_viewport(state.timeline_frames);
        let frame = if edge == TrimEdge::Right {
            c.start() + 6000
        } else {
            c.start() + c.duration() - 6000
        };
        point(
            px(lane.left
                + ((frame - viewport.start) as f64 / viewport.length as f64 * f64::from(lane.width))
                    as f32),
            down.y,
        )
    });
    visual.simulate_mouse_move(target, MouseButton::Left, shift);
    visual.run_until_parked();
    let desired = editor.read_with(&visual, |v, _| {
        let d = v
            .active_trim_drag
            .as_ref()
            .expect("real held Shift capture");
        assert!(d.stretch_modifier.get());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        d.preview.borrow().as_ref().unwrap().clip.clone()
    });
    visual.simulate_mouse_up(target, MouseButton::Left, shift);
    visual.run_until_parked();
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        eprintln!(
            "capability edge={edge:?} desired={}..{} actual={:?} error={:?} submitted={:?}",
            desired.start(),
            desired.start() + desired.duration(),
            p.clip(clip).map(|c| (c.start(), c.duration())),
            v.error,
            v.last_enqueued_edit
        );
        assert!(
            v.error.is_none(),
            "visible preview must be prepared-capable: {:?}",
            v.error
        );
        assert_eq!(p.clip(clip), Some(&desired));
        assert!(
            desired.duration() >= 12000,
            "same prepared 1/8 bound, not a positive-only preview"
        );
        assert!(desired.duration() < 96000);
        p.snapshot()
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
    });
}

fn point_for_frame(
    editor: &Entity<EditorView>,
    visual: &VisualTestContext,
    frame: u64,
    y: gpui::Pixels,
) -> gpui::Point<gpui::Pixels> {
    editor.read_with(visual, |view, _| {
        let drag = view.active_trim_drag.as_ref().unwrap();
        let lane = view.drag_lanes[&drag.source_track];
        let state = view.state.as_ref().unwrap();
        let viewport = view.visible_viewport(state.timeline_frames);
        point(
            px(lane.left
                + ((frame - viewport.start) as f64 / viewport.length as f64 * f64::from(lane.width))
                    as f32),
            y,
        )
    })
}

#[gpui::test]
fn validated_reversal_and_final_mouseup_commit_exact_command_across_pitch_and_modes(
    cx: &mut TestAppContext,
) {
    for (edge, pitch, mode) in [
        (TrimEdge::Right, 1_200, hex_project::WarpMode::Complex),
        (TrimEdge::Left, -1_200, hex_project::WarpMode::Complex),
        (TrimEdge::Right, 2_400, hex_project::WarpMode::RePitch),
        (TrimEdge::Left, 2_400, hex_project::WarpMode::RePitch),
    ] {
        let (_files, handle, editor, clip) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        editor.update(&mut visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            let settings = hex_project::default_audio_warp(
                state.project.clip(clip).unwrap(),
                state.project.tempo_map(),
                mode,
            )
            .unwrap();
            assert!(view.enqueue(Request::Edit(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings)
            })));
        });
        pump(&editor, &mut visual);
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::SetAudioClipPitch {
                clip,
                cents: pitch
            })));
        });
        pump(&editor, &mut visual);
        visual.run_until_parked();
        let before = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        let name = if edge == TrimEdge::Right {
            "right"
        } else {
            "left"
        };
        let selector = Box::leak(format!("clip-trim-{name}-{}", clip.value()).into_boxed_str());
        let down = visual.debug_bounds(selector).unwrap().center();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, shift);
        visual.simulate_mouse_move(
            point(
                down.x + px(if edge == TrimEdge::Right { -12.0 } else { 12.0 }),
                down.y,
            ),
            MouseButton::Left,
            shift,
        );
        visual.run_until_parked();
        let invalid = point_for_frame(
            &editor,
            &visual,
            if edge == TrimEdge::Right {
                96_100
            } else {
                191_900
            },
            down.y,
        );
        visual.simulate_mouse_move(invalid, MouseButton::Left, shift);
        visual.run_until_parked();
        let checks = editor.read_with(&visual, |view, _| {
            let drag = view.active_trim_drag.as_ref().unwrap();
            let shown = drag.preview.borrow();
            hex_engine::audio_transform::validate_clip_warp_preview(
                &shown.as_ref().unwrap().clip,
                view.state.as_ref().unwrap().project.tempo_map(),
            )
            .unwrap();
            drag.resolution.borrow().point_checks
        });
        visual.simulate_mouse_move(invalid, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.active_trim_drag
                    .as_ref()
                    .unwrap()
                    .resolution
                    .borrow()
                    .point_checks,
                checks
            )
        });
        let reverse = point_for_frame(&editor, &visual, 144_000, down.y);
        visual.simulate_mouse_move(reverse, MouseButton::Left, shift);
        visual.run_until_parked();
        let release = point_for_frame(
            &editor,
            &visual,
            if edge == TrimEdge::Right {
                168_000
            } else {
                120_000
            },
            down.y,
        );
        let (expected, frame) = editor.read_with(&visual, |view, _| {
            let drag = view.active_trim_drag.as_ref().unwrap();
            let project = &view.state.as_ref().unwrap().project;
            let lane = view.drag_lanes[&drag.source_track];
            let viewport = view.visible_viewport(view.state.as_ref().unwrap().timeline_frames);
            let frame =
                super::snapped_edge_frame(drag, project, f32::from(release.x), lane, viewport)
                    .unwrap();
            let expected = hex_project::preview_audio_clip_stretch(
                &drag.clip,
                edge,
                frame,
                project.tempo_map(),
                drag.asset_playback_frames,
            )
            .unwrap();
            hex_engine::audio_transform::validate_clip_warp_preview(&expected, project.tempo_map())
                .unwrap();
            assert_ne!(
                drag.preview.borrow().as_ref().unwrap().clip,
                expected,
                "final MouseUp must be resolved separately"
            );
            (expected, frame)
        });
        visual.simulate_mouse_up(release, MouseButton::Left, shift);
        visual.run_until_parked();
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(view.state.as_ref().unwrap().project.clip(clip), Some(&expected));
            assert!(matches!(view.last_enqueued_edit, Some(Command::StretchAudioClipEdge { frame: actual, .. }) if actual == frame));
            view.state.as_ref().unwrap().project.snapshot()
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
        });
    }
}

#[gpui::test]
fn large_invalid_expansion_releases_last_validated_frame_both_edges(cx: &mut TestAppContext) {
    // This is the pointer/admission/ACK boundary, not a long-source DSP profile.
    // Scale source, placement and viewport together: identical pixel geometry
    // and the real 8x admission limit, with at most 32k prepared output frames.
    let sample_rate = 8_000;
    let source_frames = sample_rate / 2;
    let frames = |at_48k: u64| at_48k * u64::from(sample_rate) / (48_000 * 4);
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        let (_files, handle, editor, clip) =
            fixture_with_source_frames(cx, sample_rate, source_frames);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        if edge == TrimEdge::Left {
            editor.update(&mut visual, |view, _| {
                let track = view.state.as_ref().unwrap().project.tracks()[0].id();
                assert!(view.enqueue(Request::Edit(Command::MoveClip {
                    clip,
                    track,
                    start: frames(1_000_000)
                })));
            });
            pump(&editor, &mut visual);
        }
        editor.update(&mut visual, |view, _| {
            let track = view.state.as_ref().unwrap().project.tracks()[0].id();
            assert!(view.enqueue(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::from_quarters(128).unwrap(),
                length: hex_project::BeatTime::ONE,
            })));
        });
        pump(&editor, &mut visual);
        editor.update(&mut visual, |view, cx| {
            view.viewport = Some(super::Viewport {
                start: 0,
                length: frames(1_400_000),
            });
            cx.notify();
        });
        visual.run_until_parked();
        let before = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        let name = if edge == TrimEdge::Right {
            "right"
        } else {
            "left"
        };
        let selector = Box::leak(format!("clip-trim-{name}-{}", clip.value()).into_boxed_str());
        let down = visual.debug_bounds(selector).unwrap().center();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, shift);
        visual.simulate_mouse_move(
            point(
                down.x + px(if edge == TrimEdge::Right { 12.0 } else { -12.0 }),
                down.y,
            ),
            MouseButton::Left,
            shift,
        );
        visual.run_until_parked();
        let target = point_for_frame(
            &editor,
            &visual,
            if edge == TrimEdge::Right {
                frames(1_000_000)
            } else {
                frames(100_000)
            },
            down.y,
        );
        editor.read_with(&visual, |view, _| {
            let drag = view.active_trim_drag.as_ref().unwrap();
            assert_eq!(
                view.hovered_drag_target(f32::from(target.x), f32::from(target.y))
                    .map(|(track, _)| track),
                Some(drag.source_track),
                "upper-bound test must actually release in its Arrangement lane"
            );
        });
        visual.simulate_mouse_move(target, MouseButton::Left, shift);
        visual.run_until_parked();
        let expected = editor.read_with(&visual, |view, _| {
            let drag = view.active_trim_drag.as_ref().unwrap();
            let preview = drag.preview.borrow().as_ref().unwrap().clone();
            assert!(
                preview.clip.duration() > u64::from(source_frames),
                "the retained candidate must exercise a real expansion"
            );
            assert!(
                preview.clip.duration() <= u64::from(source_frames) * 8,
                "unsupported expansion cannot become visible"
            );
            hex_engine::audio_transform::validate_clip_warp_preview(
                &preview.clip,
                view.state.as_ref().unwrap().project.tempo_map(),
            )
            .unwrap();
            preview
        });
        visual.simulate_mouse_up(target, MouseButton::Left, shift);
        visual.run_until_parked();
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(view.state.as_ref().unwrap().project.clip(clip), Some(&expected.clip));
            assert!(matches!(view.last_enqueued_edit, Some(Command::StretchAudioClipEdge { frame, .. }) if frame == expected.command_frame));
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
}
