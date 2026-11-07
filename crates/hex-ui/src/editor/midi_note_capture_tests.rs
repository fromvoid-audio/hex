//! Single/group held input; accepted geometry stays with the existing receipt.
use super::*;
use gpui::{CursorStyle, Render, div, prelude::*};
use std::{cell::Cell, rc::Rc, sync::Arc};
struct Host {
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
}
impl Render for Host {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
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
                .cursor(CursorStyle::PointingHand)
                .on_mouse_down(MouseButton::Right, move |_, _, _| down.set(down.get() + 1))
                .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(up.get() + 1))
                .on_scroll_wheel(move |_, _, _| wheel.set(wheel.get() + 1)),
        )
    }
}
struct Fixture {
    visual: VisualTestContext,
    editor: Entity<EditorView>,
    actions: Rc<Cell<usize>>,
    down: gpui::Point<gpui::Pixels>,
    end: gpui::Point<gpui::Pixels>,
    _environment: Arc<crate::editor::test_environment::Environment>,
}
fn open(cx: &mut TestAppContext, group: bool, copy: bool, edge: i8) -> Fixture {
    let (mut visual, editor, environment, clip) = fixture(cx);
    seed(&editor, &mut visual, clip);
    editor.update(&mut visual, |v, cx| {
        let ids: Vec<_> = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes()
            .iter()
            .map(|n| n.id())
            .collect();
        v.midi_selection.set_single(ids[0]);
        if group {
            v.midi_selection.toggle(ids[1]).unwrap();
        }
        cx.notify();
    });
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
        });
    });
    visual.simulate_resize(gpui::size(px(1280.), px(800.)));
    visual.run_until_parked();
    let quarter =
        position(&editor, &mut visual, 1., 60).x - position(&editor, &mut visual, 0., 60).x;
    let down = match edge {
        -1 => position(&editor, &mut visual, 0.5, 60) + point(px(1.), px(0.)),
        1 => position(&editor, &mut visual, 1., 60) - point(px(1.), px(0.)),
        _ => position(&editor, &mut visual, 0.75, 60),
    };
    let end = down + point(quarter * if copy { 0.5 } else { 0.25 }, px(0.));
    Fixture {
        visual,
        editor,
        actions,
        down,
        end,
        _environment: environment,
    }
}
fn modifiers(copy: bool) -> Modifiers {
    Modifiers {
        alt: copy,
        ..Modifiers::default()
    }
}
fn cursor(copy: bool, edge: i8) -> CursorStyle {
    match edge {
        _ if copy => CursorStyle::DragCopy,
        -1 => CursorStyle::ResizeLeft,
        1 => CursorStyle::ResizeRight,
        _ => CursorStyle::ClosedHand,
    }
}
#[gpui::test]
fn note_move_edges_group_and_copy_own_input_then_release_to_receipt(cx: &mut TestAppContext) {
    for (group, copy, edge) in [
        (false, false, 0),
        (false, false, -1),
        (false, false, 1),
        (true, false, 0),
        (true, false, -1),
        (true, false, 1),
        (false, true, 0),
        (true, true, 0),
        (false, true, -1),
        (false, true, 1),
        (true, true, -1),
        (true, true, 1),
    ] {
        let cx = &mut cx.new_app();
        let Fixture {
            mut visual,
            editor,
            actions,
            down,
            end,
            _environment,
        } = open(cx, group, copy, edge);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let mods = modifiers(copy);
        visual.simulate_mouse_down(down, MouseButton::Left, mods);
        assert!(
            editor.read_with(&visual, |v, _| v.midi_note_gesture.is_some()
                || v.midi_group_gesture.is_some())
        );
        let outside = point(px(640.), px(10.));
        visual.simulate_mouse_down(outside, MouseButton::Right, mods);
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: outside,
            delta: gpui::ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(
            actions.get(),
            0,
            "group={group} copy={copy} edge={edge}: input is exclusive before motion"
        );
        visual.simulate_mouse_move(point(end.x, outside.y), MouseButton::Left, mods);
        visual.run_until_parked();
        assert_eq!(visual.rendered_cursor_style(), Some(cursor(copy, edge)));
        visual.simulate_mouse_move(end, MouseButton::Left, mods);
        repaint(&editor, &mut visual);
        let held = bodies(&editor, &mut visual);
        visual.simulate_mouse_up(end, MouseButton::Left, mods);
        visual.simulate_mouse_up(outside, MouseButton::Right, mods);
        let reply = terminal(&editor, &mut visual);
        assert!(reply.error.is_none());
        for _ in 0..3 {
            repaint(&editor, &mut visual);
            assert_eq!(bodies(&editor, &mut visual), held);
            assert!(!visual.update(|w, _| w.is_pointer_dragging()));
            editor.read_with(&visual, |v, _| {
                assert!(v.midi_note_gesture.is_none() && v.midi_group_gesture.is_none());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
        assert_eq!(actions.get(), 0);
        visual.simulate_mouse_down(outside, MouseButton::Right, mods);
        assert_eq!(actions.get(), 1);
        editor.update(&mut visual, |v, cx| {
            v.receive(reply);
            cx.notify();
        });
        repaint(&editor, &mut visual);
        assert_eq!(bodies(&editor, &mut visual), held);
        editor.read_with(&visual, |v, _| assert!(v.pending_midi_drag.is_none()));
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let actual = v.state.as_ref().unwrap().project.snapshot();
            let mut expected = before;
            assert!(actual.next_id >= expected.next_id);
            expected.next_id = actual.next_id;
            assert_eq!(actual, expected);
        });
    }
}
#[gpui::test]
fn note_input_loss_discards_matching_draft_without_stale_edit_or_late_up(cx: &mut TestAppContext) {
    for group in [false, true] {
        for loss in [
            "button",
            "focus",
            "invalid",
            "escape",
            "generation",
            "revision",
            "hidden",
            "bounds",
            "window",
            "outside",
        ] {
            let cx = &mut cx.new_app();
            let Fixture {
                mut visual,
                editor,
                actions,
                down,
                end,
                _environment,
            } = open(cx, group, false, 0);
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let outside = point(px(640.), px(10.));
            match loss {
                "button" => visual.simulate_mouse_move(outside, None, Modifiers::default()),
                "escape" => visual.simulate_keystrokes("escape"),
                "generation" => editor.update(&mut visual, |v, cx| {
                    v.project_generation += 1;
                    cx.notify();
                }),
                "revision" => editor.update(&mut visual, |v, cx| {
                    let state = v.state.as_ref().unwrap();
                    v.state = Some(Arc::new(hex_session::service::ViewState {
                        canonical_audio_source: state.canonical_audio_source.clone(),
                        comparison_audio: None,
                        project: Arc::clone(&state.project),
                        waveforms: state.waveforms.clone(),
                        source_overviews: state.source_overviews.clone(),
                        audio_clip_waveforms: state.audio_clip_waveforms.clone(),
                        midi_preview: Arc::clone(&state.midi_preview),
                        gain_preview: state.gain_preview.clone(),
                        timeline_frames: state.timeline_frames,
                        can_undo: state.can_undo,
                        can_redo: state.can_redo,
                        revision: state.revision + 1,
                        unsaved: state.unsaved,
                        document_path: state.document_path.clone(),
                    }));
                    cx.notify();
                }),
                "hidden" => editor.update(&mut visual, |v, cx| {
                    v.clip_view_open = false;
                    cx.notify();
                }),
                "bounds" => visual.simulate_resize(gpui::size(px(1000.), px(700.))),
                "window" => visual.deactivate_window(),
                "outside" => {
                    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default())
                }
                "focus" | "invalid" => visual.update(|w, cx| {
                    if loss == "focus" {
                        editor.update(cx, |v, cx| v.search_browser(w, cx));
                    } else {
                        w.dispatch_input(
                            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                                position: point(end.x, px(f32::NAN)),
                                pressed_button: Some(MouseButton::Left),
                                modifiers: Modifiers::default(),
                            }),
                            cx,
                        );
                    }
                    w.dispatch_input(
                        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                            position: end,
                            button: MouseButton::Left,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                        }),
                        cx,
                    );
                }),
                _ => unreachable!(),
            }
            visual.run_until_parked();
            if !matches!(loss, "focus" | "invalid" | "outside") {
                visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
            }
            visual.run_until_parked();
            assert_eq!(actions.get(), 0, "group={group}/{loss}: trailing action");
            editor.read_with(&visual, |v, _| {
                assert!(v.midi_note_gesture.is_none() && v.midi_group_gesture.is_none());
                assert!(v.pending_midi_drag.is_none());
                assert!(!v.edit_pending());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
    }
}
#[gpui::test]
#[ignore = "opt-in single/group note dispatch and repaint CPU probe"]
fn note_input_dispatch_repaint_latency(cx: &mut TestAppContext) {
    for group in [false, true] {
        let cx = &mut cx.new_app();
        let Fixture {
            mut visual,
            down,
            end,
            _environment,
            ..
        } = open(cx, group, false, 0);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        let mut samples = Vec::new();
        for n in 0..144 {
            let at = down + (end - down) * (1. + (n % 8) as f32 / 8.);
            let start = Instant::now();
            visual.simulate_mouse_move(at, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            if n >= 16 {
                samples.push(start.elapsed().as_secs_f64() * 1_000_000.);
            }
        }
        samples.sort_by(f64::total_cmp);
        visual.simulate_mouse_up(
            point(end.x, px(10.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        println!(
            "note_input_dispatch_us group={group} p50={} p95={} max={} window=1280x800 tracks=1 clips=1 notes=3 selected={} warmups=16 samples=128",
            samples[64],
            samples[121],
            samples[127],
            if group { 2 } else { 1 }
        );
    }
}
