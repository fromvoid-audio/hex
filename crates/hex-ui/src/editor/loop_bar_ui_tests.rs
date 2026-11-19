//! Real canonical worker + rendered loop-bar gestures, not user app interaction.
use super::super::{EditorView, musical_time::SnapGrid, viewport::Viewport};
use gpui::{AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point};
use hex_project::{BeatTime, Command, LoopRegion, Tempo};
use hex_session::service::{Event, Request, Service};
use std::time::Duration;
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
            std::time::Instant::now() < until,
            "pump: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn fixture(
    cx: &mut TestAppContext,
    start: u64,
    end: u64,
    viewport: Viewport,
) -> (gpui::AnyWindowHandle, Entity<EditorView>) {
    cx.update(super::super::bind_editor_keys);
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "keep contents".into(),
        }))
        .unwrap();
    let event = next(&service);
    let track = event.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_raw(16 << 32),
        }))
        .unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::SetLoopRegion {
            start,
            end,
            enabled: false,
        }))
        .unwrap();
    let event = next(&service);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.activate_window();
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline loop bar test".into()))
            });
            editor.update(cx, |v, cx| {
                v.state = Some(event.state);
                v.viewport = Some(viewport);
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor)
}
fn at(visual: &mut VisualTestContext, fraction: f32) -> gpui::Point<gpui::Pixels> {
    let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
    point(
        rail.left() + rail.size.width * fraction,
        rail.top() + rail.size.height / 2.0,
    )
}
fn region(editor: &Entity<EditorView>, visual: &VisualTestContext) -> LoopRegion {
    editor.read_with(visual, |v, _| {
        v.state.as_ref().unwrap().project.loop_region().unwrap()
    })
}
fn click(visual: &mut VisualTestContext, p: gpui::Point<gpui::Pixels>, modifiers: Modifiers) {
    visual.simulate_mouse_down(p, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(p, MouseButton::Left, modifiers);
    visual.run_until_parked();
}
#[gpui::test]
fn command_click_toggles_both_ways_and_preserves_content_history(cx: &mut TestAppContext) {
    let (handle, editor) = fixture(
        cx,
        48_000,
        144_000,
        Viewport {
            start: 0,
            length: 384_000,
        },
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let p = at(&mut visual, 0.25);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    for enabled in [true, false, true] {
        click(&mut visual, p, command);
        pump(&editor, &mut visual);
        let r = region(&editor, &visual);
        assert_eq!(
            (r.start(), r.end(), r.enabled()),
            (48_000, 144_000, enabled)
        );
    }
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.snapshot(),
            before,
            "one Undo reverses one toggle and contents persist"
        )
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let snapshot = v.state.as_ref().unwrap().project.snapshot();
        assert_eq!(
            hex_project::Project::from_snapshot(snapshot.clone())
                .unwrap()
                .snapshot(),
            snapshot
        );
    });
}
#[gpui::test]
fn body_drag_and_shift_steps_preserve_full_length_original_origin_and_one_undo(
    cx: &mut TestAppContext,
) {
    for viewport in [
        Viewport {
            start: 0,
            length: 384_000,
        },
        Viewport {
            start: 24_000,
            length: 192_000,
        },
    ] {
        let (handle, editor) = fixture(cx, 48_000, 96_000, viewport);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let fraction = (72_000 - viewport.start) as f32 / viewport.length as f32;
        let down = at(&mut visual, fraction);
        let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
        let step_px = rail.size.width * (48_000.0 / viewport.length as f32);
        let target = point(down.x + step_px * 1.2, down.y);
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_mouse_down(down, MouseButton::Left, shift);
        visual.simulate_mouse_move(target, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            let gesture = v.loop_rail_gesture.expect("body owns whole-range drag");
            let p = gesture.preview.unwrap();
            assert_eq!((p.start(), p.end(), p.enabled()), (96_000, 144_000, false));
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        visual.simulate_mouse_up(target, MouseButton::Left, shift);
        pump(&editor, &mut visual);
        assert_eq!(region(&editor, &visual).start(), 96_000);
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        // Shift uses the original origin in the reverse direction, with a zero lower bound.
        let down = at(&mut visual, fraction);
        let target = point(down.x - step_px * 1.2, down.y);
        visual.simulate_mouse_down(down, MouseButton::Left, shift);
        visual.simulate_mouse_move(target, MouseButton::Left, shift);
        visual.simulate_mouse_up(target, MouseButton::Left, shift);
        pump(&editor, &mut visual);
        let r = region(&editor, &visual);
        assert_eq!((r.start(), r.end()), (0, 48_000));
    }
}
#[gpui::test]
fn musical_binary_marker_supports_fractional_durations_and_ignores_view_width(
    cx: &mut TestAppContext,
) {
    for (raw, expected) in [
        (2_u64 << 32, true),
        (4 << 32, true),
        (3 << 32, false),
        (1 << 31, true),
        (1 << 30, true),
    ] {
        let end = (raw as u128 * 24_000 / (1u128 << 32)) as u64;
        let (handle, editor) = fixture(
            cx,
            0,
            end,
            Viewport {
                start: 0,
                length: 384_000,
            },
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("loop-binary-length-marker").is_some(),
            expected,
            "rawbeatduration={raw}"
        );
        editor.update(&mut visual, |v, cx| {
            v.viewport = Some(Viewport {
                start: 0,
                length: 192_000,
            });
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("loop-binary-length-marker").is_some(),
            expected
        );
    }
}

#[gpui::test]
fn ordinary_body_move_preserves_length_preview_commit_undo_redo_and_save_open(
    cx: &mut TestAppContext,
) {
    for grid in [SnapGrid::Off, SnapGrid::Quarter, SnapGrid::Adaptive] {
        let (handle, editor) = fixture(
            cx,
            48_000,
            120_000,
            Viewport {
                start: 0,
                length: 384_000,
            },
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |v, cx| {
            v.snap_grid = grid;
            cx.notify();
        });
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let down = at(&mut visual, 84_000.0 / 384_000.0);
        // A plain body click is not an enable action or an edit.
        click(&mut visual, down, Modifiers::default());
        editor.read_with(&visual, |v, _| {
            assert!(v.loop_rail_gesture.is_none());
            assert!(v.pending_requests.is_empty());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        let width = visual
            .debug_bounds("arrangement-loop-rail")
            .unwrap()
            .size
            .width;
        let target = point(down.x + width * (30_000.0 / 384_000.0), down.y);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        let shown = editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            let shown = v.loop_rail_gesture.unwrap().preview.unwrap();
            assert_eq!(shown.end() - shown.start(), 72_000);
            assert!(!shown.enabled());
            match grid {
                SnapGrid::Off => assert!(shown.start().abs_diff(78_000) <= 1),
                SnapGrid::Quarter => assert_eq!(shown.start(), 72_000),
                _ => assert!(shown.start() > 48_000),
            }
            shown
        });
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        assert_eq!(region(&editor, &visual), shown);
        let after = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let revision = editor.update(&mut visual, |v, _| {
            v.last_enqueued_edit = None;
            v.state.as_ref().unwrap().revision
        });
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(
                v.last_enqueued_edit.is_none(),
                "duplicate release cannot enqueue an edit"
            );
            assert!(v.loop_rail_gesture.is_none());
            assert_eq!(v.state.as_ref().unwrap().revision, revision);
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after);
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
        if grid == SnapGrid::Off {
            let directory = super::super::test_environment::Environment::new().unwrap();
            let path = directory.root().join("loop.hex");
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::SaveNew(path.clone())))
            });
            pump(&editor, &mut visual);
            assert!(path.is_file());
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Open(path.clone())))
            });
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after);
                assert!(v.loop_rail_gesture.is_none());
            });
        }
    }
}

#[gpui::test]
fn shift_steps_reverse_from_initial_origin_across_grids_and_final_mouse_up(
    cx: &mut TestAppContext,
) {
    for (viewport, grid) in [
        (
            Viewport {
                start: 0,
                length: 384_000,
            },
            SnapGrid::Quarter,
        ),
        (
            Viewport {
                start: 24_000,
                length: 192_000,
            },
            SnapGrid::Sixteenth,
        ),
        (
            Viewport {
                start: 0,
                length: 384_000,
            },
            SnapGrid::Off,
        ),
    ] {
        let (handle, editor) = fixture(cx, 60_000, 96_000, viewport);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |v, cx| {
            v.snap_grid = grid;
            cx.notify();
        });
        visual.run_until_parked();
        let down = at(
            &mut visual,
            (78_000 - viewport.start) as f32 / viewport.length as f32,
        );
        let width = visual
            .debug_bounds("arrangement-loop-rail")
            .unwrap()
            .size
            .width;
        let step = width * (36_000.0 / viewport.length as f32);
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, shift);
        for (steps, expected) in [(1.2, 96_000), (-1.2, 24_000), (2.2, 132_000), (0.2, 60_000)] {
            let target = point(down.x + step * steps, down.y);
            visual.simulate_mouse_move(target, MouseButton::Left, shift);
            editor.read_with(&visual, |v, _| {
                let shown = v.loop_rail_gesture.unwrap().preview.unwrap();
                assert_eq!((shown.start(), shown.end()), (expected, expected + 36_000));
            });
        }
        // MouseUp at a new position resolves from the same capture, not last Move.
        let final_point = point(down.x + step * 1.2, down.y);
        visual.simulate_mouse_up(final_point, MouseButton::Left, shift);
        pump(&editor, &mut visual);
        let actual = region(&editor, &visual);
        assert_eq!((actual.start(), actual.end()), (96_000, 132_000));
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        assert_eq!(
            region(&editor, &visual),
            LoopRegion::new(60_000, 96_000, false).unwrap()
        );
    }
}

#[gpui::test]
fn loop_move_modifier_changes_cancel_and_focus_loss_do_not_submit(cx: &mut TestAppContext) {
    for cancel in ["escape", "blur", "alt", "control", "open"] {
        let (handle, editor) = fixture(
            cx,
            48_000,
            96_000,
            Viewport {
                start: 0,
                length: 384_000,
            },
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let directory = super::super::test_environment::Environment::new().unwrap();
        let path = directory.root().join("captured-loop.hex");
        if cancel == "open" {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::SaveNew(path.clone())))
            });
            pump(&editor, &mut visual);
        }
        let down = at(&mut visual, 72_000.0 / 384_000.0);
        let target = at(&mut visual, 110_000.0 / 384_000.0);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        assert!(editor.read_with(&visual, |v, _| v.loop_rail_gesture.is_some()));
        editor.update(&mut visual, |v, _| v.last_enqueued_edit = None);
        let mut modifiers = Modifiers::default();
        match cancel {
            "escape" => visual.simulate_keystrokes("escape"),
            "blur" => visual.update(|window, _| window.blur()),
            "alt" => {
                modifiers.alt = true;
                visual.simulate_modifiers_change(modifiers);
            }
            "open" => {
                editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(Request::Open(path.clone())))
                });
                pump(&editor, &mut visual);
            }
            _ => {
                modifiers.control = true;
                visual.simulate_modifiers_change(modifiers);
            }
        }
        visual.simulate_mouse_up(target, MouseButton::Left, modifiers);
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(
                v.last_enqueued_edit.is_none(),
                "cancelled/late release cannot enqueue: {cancel}"
            );
            assert!(v.loop_rail_gesture.is_none(), "{cancel}");
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                before,
                "{cancel}"
            );
            assert!(v.pending_requests.is_empty() && v.worker_requests_pending == 0);
        });
    }
}

#[gpui::test]
fn marker_updates_after_actual_edge_resize_and_rounded_tempo_change(cx: &mut TestAppContext) {
    let (handle, editor) = fixture(
        cx,
        0,
        48_000,
        Viewport {
            start: 0,
            length: 384_000,
        },
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("loop-binary-length-marker").is_some());
    let start = at(&mut visual, 48_000.0 / 384_000.0);
    let end = at(&mut visual, 72_000.0 / 384_000.0);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    assert!(visual.debug_bounds("loop-binary-length-marker").is_none());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    assert_eq!(region(&editor, &visual).end(), 72_000);
    assert!(visual.debug_bounds("loop-binary-length-marker").is_none());
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(137_000_000).unwrap(),
        })))
    });
    pump(&editor, &mut visual);
    let (start_frame, end_frame) = editor.read_with(&visual, |v, _| {
        let map = v.state.as_ref().unwrap().project.tempo_map();
        (
            map.beat_to_frame(BeatTime::from_raw(1 << 31)).unwrap(),
            map.beat_to_frame(BeatTime::from_raw((1 << 31) + (2 << 32)))
                .unwrap(),
        )
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetLoopRegion {
            start: start_frame,
            end: end_frame,
            enabled: false,
        })))
    });
    pump(&editor, &mut visual);
    assert!(visual.debug_bounds("loop-binary-length-marker").is_some());
}

#[gpui::test]
fn moving_body_recalculates_modifiers_without_changing_initial_origin(cx: &mut TestAppContext) {
    let (handle, editor) = fixture(
        cx,
        60_000,
        96_000,
        Viewport {
            start: 0,
            length: 384_000,
        },
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, cx| {
        v.snap_grid = SnapGrid::Off;
        cx.notify();
    });
    visual.run_until_parked();
    let down = at(&mut visual, 78_000.0 / 384_000.0);
    let target = at(&mut visual, 98_000.0 / 384_000.0);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    let ordinary = editor.read_with(&visual, |v, _| {
        v.loop_rail_gesture.unwrap().preview.unwrap()
    });
    assert!(ordinary.start().abs_diff(80_000) <= 1);
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    visual.simulate_modifiers_change(shift);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.loop_rail_gesture.unwrap().preview.unwrap(),
            LoopRegion::new(96_000, 132_000, false).unwrap()
        );
    });
    visual.simulate_modifiers_change(Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.loop_rail_gesture.unwrap().preview.unwrap(), ordinary)
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    assert_eq!(region(&editor, &visual), ordinary);
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    assert_eq!(
        region(&editor, &visual),
        LoopRegion::new(60_000, 96_000, false).unwrap()
    );
}

// Review 4319218033: actual captured pointer travel must disqualify a click,
// including a final release not preceded by a move and a return to the origin.
fn command_vertical_motion_control(cx: &mut TestAppContext, moves: &[f32], release_y: f32) {
    let (handle, editor) = fixture(
        cx,
        48_000,
        144_000,
        Viewport {
            start: 0,
            length: 384_000,
        },
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let down = at(&mut visual, 0.25);
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_mouse_down(down, MouseButton::Left, command);
    assert!(editor.read_with(&visual, |v, _| v.loop_rail_gesture.is_some()));
    for y in moves {
        visual.simulate_mouse_move(
            point(down.x, down.y + gpui::px(*y)),
            MouseButton::Left,
            command,
        );
        visual.run_until_parked();
        assert!(editor.read_with(&visual, |v, _| v.loop_rail_gesture.is_some()));
    }
    let up = point(down.x, down.y + gpui::px(release_y));
    eprintln!("command vertical down={down:?} moves={moves:?} actual mouse up={up:?}");
    visual.simulate_mouse_up(up, MouseButton::Left, command);
    visual.run_until_parked();
    assert!(editor.read_with(&visual, |v, _| v.loop_rail_gesture.is_none()));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.snapshot(),
            before,
            "vertical command travel is not a click and must not author a toggle"
        );
        assert_eq!(v.worker_requests_pending, 0);
        assert!(v.pending_requests.is_empty());
    });
}
#[gpui::test]
fn command_vertical_motion_outside_is_not_toggle(cx: &mut TestAppContext) {
    command_vertical_motion_control(cx, &[80.0], 80.0);
}
#[gpui::test]
fn command_vertical_motion_final_release_is_not_toggle(cx: &mut TestAppContext) {
    command_vertical_motion_control(cx, &[], 80.0);
}
#[gpui::test]
fn command_vertical_motion_return_to_origin_is_not_toggle(cx: &mut TestAppContext) {
    command_vertical_motion_control(cx, &[80.0, 0.0], 0.0);
}

#[gpui::test]
fn command_click_stationary_and_rectangular_three_pixel_jitter_still_toggle(
    cx: &mut TestAppContext,
) {
    for (dx, dy) in [(0.0, 0.0), (0.0, 3.0), (0.0, -3.0), (3.0, 3.0)] {
        let (handle, editor) = fixture(
            cx,
            48_000,
            144_000,
            Viewport {
                start: 0,
                length: 384_000,
            },
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let down = at(&mut visual, 0.25);
        let target = point(down.x + gpui::px(dx), down.y + gpui::px(dy));
        let command = Modifiers {
            platform: true,
            ..Modifiers::default()
        };
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_mouse_down(down, MouseButton::Left, command);
        visual.simulate_mouse_move(target, MouseButton::Left, command);
        visual.simulate_mouse_up(target, MouseButton::Left, command);
        pump(&editor, &mut visual);
        assert_eq!(
            region(&editor, &visual),
            LoopRegion::new(48_000, 144_000, true).unwrap()
        );
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
}

#[gpui::test]
fn command_vertical_motion_threshold_crossing_then_return_stays_disqualified(
    cx: &mut TestAppContext,
) {
    command_vertical_motion_control(cx, &[3.25, 0.0], 0.0);
}

#[gpui::test]
fn ordinary_vertical_motion_off_grid_never_authors_snap_or_toggle(cx: &mut TestAppContext) {
    let (handle, editor) = fixture(
        cx,
        50_123,
        146_123,
        Viewport {
            start: 0,
            length: 384_000,
        },
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, cx| {
        v.snap_grid = SnapGrid::Quarter;
        v.last_enqueued_edit = None;
        cx.notify();
    });
    visual.run_until_parked();
    let down = at(&mut visual, 0.25);
    let target = point(down.x, down.y + gpui::px(80.0));
    let (before, revision) = editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        (state.project.snapshot(), state.revision)
    });
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    // Modifier refresh must reuse the actual last full point and cannot qualify a move.
    visual.simulate_modifiers_change(Modifiers {
        shift: true,
        ..Modifiers::default()
    });
    visual.simulate_modifiers_change(Modifiers::default());
    editor.read_with(&visual, |v, _| {
        let gesture = v.loop_rail_gesture.expect("same held rail capture");
        assert!(!gesture.moved, "Y is not horizontal range movement");
        assert!(gesture.click_disqualified);
        assert_eq!(gesture.last_pointer, target);
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert_eq!(v.state.as_ref().unwrap().revision, revision);
        assert!(
            v.last_enqueued_edit.is_none(),
            "no snap, set or toggle submitted"
        );
        assert!(v.loop_rail_gesture.is_none());
        assert_eq!(v.worker_requests_pending, 0);
        assert!(v.pending_requests.is_empty());
    });
}

#[path = "loop_rail/review_ui_tests.rs"]
mod review_ui_tests;
