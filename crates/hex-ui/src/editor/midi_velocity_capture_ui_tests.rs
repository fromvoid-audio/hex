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
    for group in [false, true] {
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
                        MouseUpEvent {
                            button: MouseButton::Left,
                            position: end,
                            ..Default::default()
                        }
                        .into(),
                        cx,
                    );
                }),
                "invalid-x" => visual.update(|w, cx| {
                    w.dispatch_input(
                        MouseMoveEvent {
                            position: point(px(f32::NAN), end.y),
                            pressed_button: Some(MouseButton::Left),
                            ..Default::default()
                        }
                        .into(),
                        cx,
                    );
                    w.dispatch_input(
                        MouseUpEvent {
                            button: MouseButton::Left,
                            position: end,
                            ..Default::default()
                        }
                        .into(),
                        cx,
                    );
                }),
                "escape" => visual.simulate_keystrokes("escape"),
                "generation" => editor.update(&mut visual, |v, cx| {
                    v.project_generation += 1;
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
                "window" => visual.simulate_window_deactivation(),
                _ => unreachable!(),
            }
            repaint(&editor, &mut visual);
            visual.simulate_mouse_up(
                point(px(640.), px(10.)),
                MouseButton::Left,
                Modifiers::default(),
            );
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
