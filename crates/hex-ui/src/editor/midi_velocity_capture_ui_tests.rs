//! Real held value input, separate from released receipt geometry.
use super::*;
use gpui::{CursorStyle, Render};
use std::{cell::Cell, rc::Rc};
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
fn host(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    group: bool,
) -> Rc<Cell<usize>> {
    editor.update(&mut *visual, |v, cx| {
        let source = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(v.detail_clip_id().unwrap())
            .unwrap();
        let ids: Vec<_> = source.notes().iter().map(|n| n.id()).collect();
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
    repaint(editor, visual);
    actions
}
#[gpui::test]
fn velocity_value_drag_owns_buttons_wheel_and_cursor_then_frees_input(cx: &mut TestAppContext) {
    for (group, outside_release) in [(false, false), (true, false), (false, true), (true, true)] {
        let cx = &mut cx.new_app();
        let (mut visual, editor, _environment, _clip) = setup(cx);
        let actions = host(&mut visual, &editor, group);
        let down = handle(&mut visual, 0.5, 37);
        let end = down - point(px(0.), px(14.));
        let outside = point(end.x, px(10.));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        visual.simulate_event(ScrollWheelEvent {
            position: outside,
            delta: ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(actions.get(), 0, "exclusive before first motion");
        visual.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
        repaint(&editor, &mut visual);
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(CursorStyle::ResizeUpDown)
        );
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        repaint(&editor, &mut visual);
        let end = if outside_release { outside } else { end };
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        repaint(&editor, &mut visual);
        let held = handles(&mut visual);
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
        let event = terminal(&editor, &mut visual);
        assert!(event.error.is_none());
        repaint(&editor, &mut visual);
        assert_eq!(handles(&mut visual), held);
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        assert_eq!(actions.get(), 0);
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        assert_eq!(actions.get(), 1);
        editor.update(&mut visual, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        repaint(&editor, &mut visual);
        assert_eq!(handles(&mut visual), held);
    }
}
#[gpui::test]
fn velocity_input_loss_cancels_only_held_value_and_suppresses_late_release(
    cx: &mut TestAppContext,
) {
    for group in [false, true] {
        for loss in [
            "button",
            "focus",
            "invalid-x",
            "escape",
            "generation",
            "revision",
            "source",
            "selection",
            "viewport",
            "hidden",
            "bounds",
            "window",
        ] {
            let cx = &mut cx.new_app();
            let (mut visual, editor, _environment, _clip) = setup(cx);
            let actions = host(&mut visual, &editor, group);
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            let down = handle(&mut visual, 0.5, 37);
            let end = down - point(px(0.), px(14.));
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
            match loss {
                "button" => visual.simulate_mouse_move(end, None, Modifiers::default()),
                "focus" => visual.update(|w, cx| {
                    editor.update(cx, |v, _| v.focus.focus(w));
                    w.dispatch_input(
                        gpui::PlatformInput::MouseUp(MouseUpEvent {
                            button: MouseButton::Left,
                            position: point(px(640.), px(10.)),
                            ..Default::default()
                        }),
                        cx,
                    );
                }),
                "invalid-x" => visual.update(|w, cx| {
                    w.dispatch_input(
                        gpui::PlatformInput::MouseMove(MouseMoveEvent {
                            position: point(px(f32::NAN), end.y),
                            pressed_button: Some(MouseButton::Left),
                            ..Default::default()
                        }),
                        cx,
                    );
                    w.dispatch_input(
                        gpui::PlatformInput::MouseUp(MouseUpEvent {
                            button: MouseButton::Left,
                            position: point(px(640.), px(10.)),
                            ..Default::default()
                        }),
                        cx,
                    );
                }),
                "escape" => visual.simulate_keystrokes("escape"),
                "generation" => editor.update(&mut visual, |v, cx| {
                    v.project_generation += 1;
                    cx.notify();
                }),
                "revision" => editor.update(&mut visual, |v, cx| {
                    let state = v.state.as_ref().unwrap();
                    v.state = Some(std::sync::Arc::new(hex_session::service::ViewState {
                        canonical_audio_source: state.canonical_audio_source.clone(),
                        comparison_audio: None,
                        project: std::sync::Arc::clone(&state.project),
                        waveforms: state.waveforms.clone(),
                        source_overviews: state.source_overviews.clone(),
                        audio_clip_waveforms: state.audio_clip_waveforms.clone(),
                        midi_preview: std::sync::Arc::clone(&state.midi_preview),
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
                "source" => editor.update(&mut visual, |v, cx| {
                    v.selected_clip = None;
                    cx.notify();
                }),
                "selection" => editor.update(&mut visual, |v, cx| {
                    v.midi_selection.clear();
                    cx.notify();
                }),
                "viewport" => editor.update(&mut visual, |v, cx| {
                    v.piano_beat_span = BeatTime::from_quarters(8).unwrap();
                    cx.notify();
                }),
                "hidden" => editor.update(&mut visual, |v, cx| {
                    v.midi_velocity.open = false;
                    cx.notify();
                }),
                "bounds" => editor.update(&mut visual, |v, cx| {
                    v.midi_velocity.height = 90.;
                    cx.notify();
                }),
                "window" => visual.deactivate_window(),
                _ => unreachable!(),
            }
            repaint(&editor, &mut visual);
            if loss != "focus" && loss != "invalid-x" {
                visual.simulate_mouse_up(
                    point(px(640.), px(10.)),
                    MouseButton::Left,
                    Modifiers::default(),
                );
            }

            assert_eq!(actions.get(), 0, "group={group}/{loss}: late release");
            editor.read_with(&visual, |v, _| {
                assert!(v.midi_velocity.gesture.is_none(), "group={group}/{loss}");
                assert!(v.pending_midi_drag.is_none());
                assert!(!v.edit_pending());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
    }
}

#[gpui::test]
#[ignore = "opt-in held/released velocity dispatch and repaint CPU probe"]
fn velocity_dispatch_repaint_latency(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = setup(cx);
    let _actions = host(&mut visual, &editor, true);
    let down = handle(&mut visual, 0.5, 37);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    for phase in ["held", "released"] {
        if phase == "released" {
            visual.simulate_mouse_up(
                down - point(px(0.), px(14.)),
                MouseButton::Left,
                Modifiers::default(),
            );
            let event = terminal(&editor, &mut visual);
            assert!(event.error.is_none());
            // Deliberately hold the real terminal through this phase's paints.
        }
        let mut samples = Vec::new();
        for n in 0..144 {
            let start = Instant::now();
            if phase == "held" {
                visual.simulate_mouse_move(
                    down - point(px(0.), px(7. + (n % 8) as f32)),
                    Some(MouseButton::Left),
                    Modifiers::default(),
                );
            }
            repaint(&editor, &mut visual);
            if n >= 16 {
                samples.push(start.elapsed().as_secs_f64() * 1_000_000.);
            }
        }
        assert_eq!(handles(&mut visual).len(), 3);
        samples.sort_by(f64::total_cmp);
        println!(
            "velocity_repaint_us phase={phase} p50={} p95={} max={} handles=3 tracks=1 clips=1 notes=3 selected=2 warmups=16 samples=128",
            samples[64], samples[121], samples[127]
        );
    }
}

#[path = "midi_velocity_size_ui_tests.rs"]
mod sizing_tests;

#[path = "midi_marquee_capture_ui_tests.rs"]
mod marquee_tests;
