//! Actual audio brace input; the ordinary clip-loop receipt remains its owner.
use super::*;
use gpui::{
    CursorStyle, Entity, Modifiers, Render, ScrollDelta, ScrollWheelEvent, TestAppContext,
    VisualTestContext, point,
};
use std::{cell::Cell, rc::Rc, sync::Arc};
struct Host {
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
}
impl Render for Host {
    fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let editor = self.editor.clone();
        let down = self.actions.clone();
        let up = self.actions.clone();
        let wheel = self.actions.clone();
        div().size_full().child(editor).child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w_full()
                .h(px(24.))
                .cursor(CursorStyle::Crosshair)
                .on_mouse_down(MouseButton::Right, move |_, _, _| down.set(down.get() + 1))
                .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(up.get() + 1))
                .on_scroll_wheel(move |_, _, _| wheel.set(wheel.get() + 1)),
        )
    }
}
struct Fixture {
    _files: Arc<crate::editor::test_environment::Environment>,
    visual: VisualTestContext,
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
    start: gpui::Point<Pixels>,
    width: Pixels,
    clip: ClipId,
}
fn open(cx: &mut TestAppContext, start_edge: bool) -> Fixture {
    let (files, handle, editor) = crate::editor::clip_drag_ui_test::upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.), px(800.)));
    let clip = editor.update(&mut visual, |v, cx| {
        let track = &v.state.as_ref().unwrap().project.tracks()[0];
        let clip = track.clips()[0].id();
        v.selected_track = Some(track.id());
        v.selected_clip = Some(clip);
        v.audio_detail.clip_view_open = true;
        v.device_panel.active = false;
        cx.notify();
        clip
    });
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
        });
        editor.read(cx).audio_detail.focus.focus(w);
    });
    visual.run_until_parked();
    let start = visual
        .debug_bounds(if start_edge {
            "audio-loop-start"
        } else {
            "audio-loop-end"
        })
        .unwrap()
        .center();
    let width = visual.debug_bounds("audio-loop-ruler").unwrap().size.width;
    Fixture {
        _files: files,
        visual,
        editor,
        actions,
        start,
        width,
        clip,
    }
}
#[gpui::test]
fn dragging_audio_scope_body_moves_both_edges_and_keeps_released_gate(cx: &mut TestAppContext) {
    assert_scope_body(cx, false);
}

#[gpui::test]
fn dragging_looping_audio_scope_body_keeps_arrangement_bounds(cx: &mut TestAppContext) {
    assert_scope_body(cx, true);
}

#[gpui::test]
fn narrow_audio_scope_prioritizes_body_at_center_and_keeps_distinguishable_edges(
    cx: &mut TestAppContext,
) {
    let Fixture {
        mut visual,
        editor,
        width,
        clip,
        ..
    } = open(cx, true);
    editor.update(&mut visual, |v, cx| {
        v.set_audio_loop_grid(crate::editor::musical_time::SnapGrid::Off);
        cx.notify();
    });
    visual.run_until_parked();

    let total_frames = editor.read_with(&visual, |v, _| {
        let root = v
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .audio_event_root();
        let (first, end) = editor_bounds(&root);
        (end - first).max(1)
    });
    let ruler_px = f32::from(width);
    let frames_for_px = |target_px: f32| -> u64 {
        ((target_px / ruler_px) * (total_frames as f32))
            .round()
            .max(600.0) as u64
    };

    // Exercise actual rendered widths below, at, and just above 14 px (including the 600-frame
    // ~3.5 px looping scope from the bug report), plus a non-looping narrow scope.
    let cases = [
        (600_u64, true, false),
        (frames_for_px(10.0), true, false),
        (frames_for_px(14.0), true, true),
        (frames_for_px(16.0), true, true),
        (frames_for_px(10.0), false, false),
    ];

    for (len_frames, looping, expect_distinguishable_edges) in cases {
        let start_frame = 24_000_u64;
        let end_frame = start_frame + len_frames;
        editor.update(&mut visual, |v, cx| {
            assert!(v.enqueue(Request::Edit(Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::ScopeRange {
                    start: start_frame,
                    end: end_frame,
                    looping,
                },
            })));
            cx.notify();
        });
        crate::editor::clip_drag_ui_test::pump_move(&editor, &mut visual);
        visual.run_until_parked();

        let body = visual.debug_bounds("audio-scope-body").unwrap();
        let drawn_px = f32::from(body.size.width);
        if len_frames == 600 {
            assert!(
                drawn_px < 6.0,
                "600-frame scope should render narrower than 6 px, got {drawn_px}"
            );
        }

        // Pressing the exact center of the narrow scope bar must capture Body and translate
        // both endpoints equally, preserving canonical length after release.
        let center = body.center();
        let target = center + point(px(48.0), px(0.0));
        visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |v, _| {
            let g = v
                .audio_detail
                .clip_loop
                .audio_gesture
                .as_ref()
                .expect("center press captures gesture");
            assert_eq!(
                g.handle,
                Handle::Body,
                "center press at drawn width {drawn_px:.2} px should capture Body"
            );
        });
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        crate::editor::clip_drag_ui_test::pump_move(&editor, &mut visual);
        visual.run_until_parked();

        let moved = editor.read_with(&visual, |v, _| {
            let root = v
                .state
                .as_ref()
                .unwrap()
                .project
                .clip(clip)
                .unwrap()
                .audio_event_root();
            (root.view_start, root.view_end)
        });
        assert!(
            moved.0 > start_frame,
            "body drag at width {drawn_px:.2} px should translate start forward: {moved:?}"
        );
        assert_eq!(
            moved.1 - moved.0,
            len_frames,
            "body drag at width {drawn_px:.2} px must preserve exact scope length"
        );
        assert_eq!(
            moved.0 - start_frame,
            moved.1 - end_frame,
            "both endpoints must translate by the same amount"
        );

        // When there is room to distinguish edges, Start and End remain independently resizable.
        if expect_distinguishable_edges {
            let start_handle = visual.debug_bounds("audio-loop-start").unwrap().center();
            let drag_left = start_handle + point(px(-24.0), px(0.0));
            visual.simulate_mouse_down(start_handle, MouseButton::Left, Modifiers::default());
            editor.read_with(&visual, |v, _| {
                assert_eq!(
                    v.audio_detail
                        .clip_loop
                        .audio_gesture
                        .as_ref()
                        .unwrap()
                        .handle,
                    Handle::Start,
                    "start edge should remain distinguishable at width {drawn_px:.2} px"
                );
            });
            visual.simulate_mouse_move(drag_left, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            visual.simulate_mouse_up(drag_left, MouseButton::Left, Modifiers::default());
            crate::editor::clip_drag_ui_test::pump_move(&editor, &mut visual);
            visual.run_until_parked();

            let after_start = editor.read_with(&visual, |v, _| {
                let root = v
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .clip(clip)
                    .unwrap()
                    .audio_event_root();
                (root.view_start, root.view_end)
            });
            assert!(after_start.0 < moved.0);
            assert_eq!(after_start.1, moved.1);

            let end_handle = visual.debug_bounds("audio-loop-end").unwrap().center();
            let drag_right = end_handle + point(px(24.0), px(0.0));
            visual.simulate_mouse_down(end_handle, MouseButton::Left, Modifiers::default());
            editor.read_with(&visual, |v, _| {
                assert_eq!(
                    v.audio_detail
                        .clip_loop
                        .audio_gesture
                        .as_ref()
                        .unwrap()
                        .handle,
                    Handle::End,
                    "end edge should remain distinguishable at width {drawn_px:.2} px"
                );
            });
            visual.simulate_mouse_move(drag_right, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            visual.simulate_mouse_up(drag_right, MouseButton::Left, Modifiers::default());
            crate::editor::clip_drag_ui_test::pump_move(&editor, &mut visual);
            visual.run_until_parked();

            let after_end = editor.read_with(&visual, |v, _| {
                let root = v
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .clip(clip)
                    .unwrap()
                    .audio_event_root();
                (root.view_start, root.view_end)
            });
            assert_eq!(after_end.0, after_start.0);
            assert!(after_end.1 > after_start.1);
        }
    }
}

#[gpui::test]
fn returning_audio_scope_edge_inside_tolerance_restores_preview_and_skips_commit(
    cx: &mut TestAppContext,
) {
    for looping in [false, true] {
        for start_edge in [true, false] {
            let cx = &mut cx.new_app();
            let Fixture {
                mut visual,
                editor,
                width,
                clip,
                ..
            } = open(cx, start_edge);
            editor.update(&mut visual, |v, cx| {
                v.set_audio_loop_grid(crate::editor::musical_time::SnapGrid::Off);
                assert!(v.enqueue(Request::Edit(Command::ResizeAudioClipView {
                    clip,
                    operation: AudioViewResizeOperation::ScopeRange {
                        start: 0,
                        end: 96_000,
                        looping,
                    },
                })));
                cx.notify();
            });
            crate::editor::clip_drag_ui_test::pump_move(&editor, &mut visual);
            visual.run_until_parked();

            let edge = visual
                .debug_bounds(if start_edge {
                    "audio-loop-start"
                } else {
                    "audio-loop-end"
                })
                .unwrap()
                .center();
            let inward = if start_edge {
                width / 8.0
            } else {
                -width / 8.0
            };

            let (before_snapshot, before_revision, before_can_undo, before_bounds) = editor
                .read_with(&visual, |v, _| {
                    let state = v.state.as_ref().unwrap();
                    let c = state.project.clip(clip).unwrap();
                    (
                        state.project.snapshot(),
                        state.revision,
                        state.can_undo,
                        (c.start(), c.duration()),
                    )
                });

            // Out-and-back drag releasing both at the exact start and inside the 3 px tolerance.
            for return_offset in [px(0.0), if start_edge { px(1.5) } else { px(-1.5) }] {
                visual.simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
                visual.simulate_mouse_move(
                    edge + point(inward, px(0.0)),
                    MouseButton::Left,
                    Modifiers::default(),
                );
                visual.run_until_parked();
                editor.read_with(&visual, |v, _| {
                    let g = v.audio_detail.clip_loop.audio_gesture.as_ref().unwrap();
                    assert_ne!(g.preview, (0, 96_000));
                });

                let returned = edge + point(return_offset, px(0.0));
                visual.simulate_mouse_move(returned, MouseButton::Left, Modifiers::default());
                visual.run_until_parked();
                editor.read_with(&visual, |v, _| {
                    let g = v.audio_detail.clip_loop.audio_gesture.as_ref().unwrap();
                    assert_eq!(g.preview, (0, 96_000));
                });

                visual.simulate_mouse_up(returned, MouseButton::Left, Modifiers::default());
                visual.run_until_parked();
                editor.read_with(&visual, |v, _| {
                    assert!(!v.clip_loop_held());
                    assert!(!v.edit_pending());
                    assert_eq!(v.worker_requests_pending, 0);
                    let state = v.state.as_ref().unwrap();
                    assert_eq!(state.revision, before_revision);
                    assert_eq!(state.can_undo, before_can_undo);
                    assert_eq!(state.project.snapshot(), before_snapshot);
                    let c = state.project.clip(clip).unwrap();
                    assert_eq!((c.start(), c.duration()), before_bounds);
                    let root = c.audio_event_root();
                    assert_eq!((root.view_start, root.view_end), (0, 96_000));
                });
            }

            // Intentional resize outside the 3 px tolerance still commits the indicated edge once.
            let target = edge + point(inward, px(0.0));
            visual.simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            crate::editor::clip_drag_ui_test::pump_move(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                let state = v.state.as_ref().unwrap();
                assert_eq!(state.revision, before_revision + 1);
                let root = state.project.clip(clip).unwrap().audio_event_root();
                if start_edge {
                    assert!(root.view_start > 0 && root.view_end == 96_000);
                } else {
                    assert!(root.view_start == 0 && root.view_end < 96_000);
                }
            });
        }
    }
}

fn assert_scope_body(cx: &mut TestAppContext, looping: bool) {
    let Fixture {
        mut visual,
        editor,
        width,
        clip,
        ..
    } = open(cx, true);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::ScopeRange {
                start: 24_000,
                end: 72_000,
                looping,
            },
        })));
    });
    crate::editor::clip_drag_ui_test::pump_move(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let selector = Box::leak(format!("clip-visual-{}", clip.value()).into_boxed_str());
    let original = visual.debug_bounds(selector).unwrap();
    let down = visual.debug_bounds("audio-scope-body").unwrap().center();
    // One full viewport of motion overshoots the retained source and clamps.
    let up = down + point(width, px(0.));
    for cancel in [true, false] {
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let held = visual.debug_bounds(selector).unwrap();
        if looping {
            assert_eq!(held, original);
        } else {
            assert!(held.left() > original.left());
            assert!(held.right() > original.right());
            // The painted edges can round to different half-pixel positions.
            assert!(f32::from(held.size.width - original.size.width).abs() <= 1.0);
        }
        editor.read_with(&visual, |v, _| {
            let g = v.audio_detail.clip_loop.audio_gesture.as_ref().unwrap();
            assert_eq!(g.preview, (48_000, 96_000));
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        if cancel {
            visual.simulate_keystrokes("escape");
        }
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        if cancel {
            assert_eq!(visual.debug_bounds(selector).unwrap(), original);
            assert!(editor.read_with(&visual, |v, _| !v.edit_pending()));
            continue;
        }
        let terminal = crate::editor::clip_editor_loop_ui_tests::hold(&editor, &mut visual);
        assert!(
            terminal.request_complete && terminal.error.is_none(),
            "{:?}",
            terminal.error
        );
        for _ in 0..3 {
            editor.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
            assert_eq!(visual.debug_bounds(selector).unwrap(), held);
        }
        editor.update(&mut visual, |v, cx| {
            v.receive(terminal);
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(selector).unwrap(), held);
    }
    let after = editor.read_with(&visual, |v, _| {
        let project = &v.state.as_ref().unwrap().project;
        let clip = project.clip(clip).unwrap();
        let start = if looping { 24_000 } else { 48_000 };
        assert_eq!((clip.start(), clip.duration()), (start, 48_000));
        let scope = clip.audio_event_root();
        assert_eq!((scope.view_start, scope.view_end), (48_000, 96_000));
        project.snapshot()
    });
    assert_ne!(after, before);
    for (key, expected) in [("cmd-z", before), ("cmd-shift-z", after)] {
        visual.simulate_keystrokes(key);
        crate::editor::clip_drag_ui_test::pump_move(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), expected);
        });
    }
}

#[gpui::test]
fn audio_loop_start_edge_keeps_input_and_pending_receipt_separate(cx: &mut TestAppContext) {
    assert_audio_loop_receipt(cx, true);
}

#[gpui::test]
fn audio_loop_end_edge_keeps_input_and_pending_receipt_separate(cx: &mut TestAppContext) {
    assert_audio_loop_receipt(cx, false);
}

fn assert_audio_loop_receipt(cx: &mut TestAppContext, start_edge: bool) {
    let cx = &mut cx.new_app();
    let Fixture {
        mut visual,
        editor,
        actions,
        start,
        width,
        clip,
        ..
    } = open(cx, start_edge);
    if !editor.read_with(&visual, |v, _| v.audio_loop_grid().is_off()) {
        visual.simulate_keystrokes("cmd-4");
    }
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let selector = Box::leak(format!("clip-visual-{}", clip.value()).into_boxed_str());
    let original_bounds = visual.debug_bounds(selector).unwrap();
    let outside = point(px(640.), px(10.));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    assert!(editor.read_with(&visual, |v, _| v.clip_loop_held()));
    visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
    visual.simulate_event(ScrollWheelEvent {
        position: outside,
        delta: ScrollDelta::Lines(point(0., 1.)),
        ..Default::default()
    });
    assert_eq!(
        actions.get(),
        0,
        "audio loop handles must isolate input before motion"
    );
    let moved = start + point(width * if start_edge { 0.125 } else { -0.125 }, px(0.));
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(if start_edge {
            CursorStyle::ResizeLeft
        } else {
            CursorStyle::ResizeRight
        })
    );
    let preview = editor.read_with(&visual, |v, _| {
        assert!(v.audio_loop_grid().is_off());
        let gesture = v.audio_detail.clip_loop.audio_gesture.as_ref().unwrap();
        assert_ne!(
            gesture.preview,
            (gesture.root.view_start, gesture.root.view_end)
        );
        gesture.preview
    });
    let held_bounds = visual.debug_bounds(selector).unwrap();
    if start_edge {
        assert!(
            held_bounds.left() > original_bounds.left(),
            "scope {preview:?}: {original_bounds:?} -> {held_bounds:?}"
        );
        assert_eq!(held_bounds.right(), original_bounds.right());
    } else {
        assert_eq!(held_bounds.left(), original_bounds.left());
        assert!(
            held_bounds.right() < original_bounds.right(),
            "scope {preview:?}: {original_bounds:?} -> {held_bounds:?}"
        );
    }
    let redraws = Rc::new(Cell::new(0));
    let _observe = visual.update(|_, app| {
        let redraws = Rc::clone(&redraws);
        app.observe(&editor, move |_, _| redraws.set(redraws.get() + 1))
    });
    visual.simulate_mouse_move(
        moved + point(px(0.), px(4.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert_eq!(redraws.get(), 0, "unchanged scope does not repaint");
    assert_eq!(visual.debug_bounds(selector), Some(held_bounds));
    visual.simulate_mouse_up(
        point(moved.x, px(10.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    let terminal = crate::editor::clip_editor_loop_ui_tests::hold(&editor, &mut visual);
    assert!(
        terminal.request_complete && terminal.error.is_none(),
        "{:?}",
        terminal.error
    );
    visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(selector).unwrap(), held_bounds);
        editor.read_with(&visual, |v, _| {
            assert!(!v.clip_loop_held());
            let root = v.pending_clip_loop_audio(clip).unwrap().audio_event_root();
            assert_eq!((root.view_start, root.view_end), preview);
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
    assert_eq!(actions.get(), 0);
    assert!(!visual.update(|w, _| w.is_pointer_dragging()));
    visual.simulate_mouse_move(outside, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
    visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
    assert_eq!(
        actions.get(),
        1,
        "pending clip-loop edit cannot hold pointer input"
    );
    editor.update(&mut visual, |v, cx| {
        v.receive(terminal);
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds(selector).unwrap(), held_bounds);
    assert!(editor.read_with(&visual, |v, _| v.pending_clip_loop_audio(clip).is_none()));
}

#[derive(Clone, Copy, Debug)]
enum Loss {
    Button,
    Focus,
    Invalid,
    Escape,
    Geometry,
    Generation,
    Hidden,
    Source,
}

#[gpui::test]
fn audio_loop_start_button_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Button);
}

#[gpui::test]
fn audio_loop_end_button_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, false, Loss::Button);
}

#[gpui::test]
fn audio_loop_focus_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Focus);
}

#[gpui::test]
fn audio_loop_invalid_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Invalid);
}

#[gpui::test]
fn audio_loop_escape_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Escape);
}

#[gpui::test]
fn audio_loop_geometry_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Geometry);
}

#[gpui::test]
fn audio_loop_generation_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Generation);
}

#[gpui::test]
fn audio_loop_hidden_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Hidden);
}

#[gpui::test]
fn audio_loop_source_loss_cannot_commit_or_leak_late_up(cx: &mut TestAppContext) {
    assert_audio_loop_loss(cx, true, Loss::Source);
}

fn assert_audio_loop_loss(cx: &mut TestAppContext, start_edge: bool, loss: Loss) {
    let cx = &mut cx.new_app();
    let Fixture {
        mut visual,
        editor,
        actions,
        start,
        width,
        ..
    } = open(cx, start_edge);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let outside = point(px(640.), px(10.));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    let moved = start + point(width * if start_edge { 0.125 } else { -0.125 }, px(0.));
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    match loss {
        Loss::Button => visual.simulate_mouse_move(outside, None, Modifiers::default()),
        Loss::Invalid => visual.simulate_mouse_move(
            point(moved.x, px(f32::NAN)),
            MouseButton::Left,
            Modifiers::default(),
        ),
        Loss::Escape => visual.simulate_keystrokes("escape"),
        Loss::Geometry => visual.simulate_resize(gpui::size(px(1100.), px(750.))),
        Loss::Generation => editor.update(&mut visual, |v, cx| {
            v.project_generation += 1;
            cx.notify();
        }),
        Loss::Hidden => editor.update(&mut visual, |v, cx| {
            v.audio_detail.clip_view_open = false;
            cx.notify();
        }),
        Loss::Source => editor.update(&mut visual, |v, cx| {
            v.audio_detail.event.source_view = true;
            cx.notify();
        }),
        Loss::Focus => visual.update(|w, cx| {
            editor.update(cx, |v, cx| v.search_browser(w, cx));
            w.dispatch_input(
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    position: outside,
                    button: MouseButton::Left,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            );
        }),
    }
    visual.run_until_parked();
    if !matches!(loss, Loss::Focus) {
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    }
    visual.run_until_parked();
    assert_eq!(actions.get(), 0, "edge={start_edge} loss={loss:?}");
    if matches!(loss, Loss::Geometry) {
        let ruler = visual.debug_bounds("audio-loop-ruler").unwrap();
        let marker = visual.debug_bounds("audio-loop-start-marker").unwrap();
        assert!(
            (marker.left() - ruler.left()).abs() < px(1.),
            "cancelled geometry must repaint canonical loop bounds"
        );
    }
    editor.read_with(&visual, |v, _| {
        assert!(!v.clip_loop_held(), "edge={start_edge} loss={loss:?}");
        assert!(!v.edit_pending(), "edge={start_edge} loss={loss:?}");
        assert!(v.pending_requests.is_empty());
        assert_eq!(v.worker_requests_pending, 0);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

/// Real retained-root preview; setup/cancellation and worker admission excluded.
#[gpui::test]
#[ignore = "manual optimized audio loop profile; no native acceptance"]
fn profile_audio_loop_pointer(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_AUDIO_LOOP_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 128 };
    for start_edge in [true, false] {
        let cx = &mut cx.new_app();
        let Fixture {
            mut visual,
            editor,
            start,
            width,
            ..
        } = open(cx, start_edge);
        let mut timings = Vec::with_capacity(samples);
        for round in 0..warmup + samples {
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert!(editor.read_with(&visual, |v, _| v.clip_loop_held()));
            let clock = std::time::Instant::now();
            let mut at = start;
            for step in 0..8 {
                let displacement = width * (((round + step) % 5) as f32 / 16.);
                at = start
                    + point(
                        if start_edge {
                            displacement
                        } else {
                            -displacement
                        },
                        px(0.),
                    );
                visual.simulate_mouse_move(at, MouseButton::Left, Modifiers::default());
                visual.run_until_parked();
            }
            let elapsed = clock.elapsed().as_secs_f64() * 1_000_000.;
            visual.simulate_keystrokes("escape");
            visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert!(!v.clip_loop_held());
                assert!(!v.edit_pending());
                assert_eq!(v.worker_requests_pending, 0);
                assert!(v.pending_requests.is_empty());
            });
            if round >= warmup {
                timings.push(elapsed);
            }
        }
        timings.sort_by(f64::total_cmp);
        let percentile = |p: usize| timings[(timings.len() - 1) * p / 100];
        let scale = visual.update(|w, _| w.scale_factor());
        println!(
            "AUDIO_LOOP_PROFILE start_edge={start_edge} width=1280 height=800 scale={scale} source_frames=96000 rate=48000 source_channels=1 tracks=2 clips=1 warmup={warmup} samples={samples} debug_smoke={smoke} completed_updates_per_sample=8 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100)
        );
    }
}
