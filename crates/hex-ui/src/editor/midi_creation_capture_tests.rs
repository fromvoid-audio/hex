//! Held creation input is distinct from the existing accepted display receipt.
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
        let right = self.actions.clone();
        let middle = self.actions.clone();
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
                .on_mouse_down(MouseButton::Right, move |_, _, _| {
                    right.set(right.get() + 1)
                })
                .on_mouse_down(MouseButton::Middle, move |_, _, _| {
                    middle.set(middle.get() + 1)
                })
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
fn open(cx: &mut TestAppContext) -> Fixture {
    let (mut visual, editor, environment, _) = fixture(cx);
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
        });
    });
    visual.simulate_resize(gpui::size(px(1280.), px(800.)));
    visual.run_until_parked();
    let down = position(&editor, &mut visual, 1., 60);
    let end = position(&editor, &mut visual, 2., 60);
    Fixture {
        visual,
        editor,
        actions,
        down,
        end,
        _environment: environment,
    }
}
#[gpui::test]
fn held_creation_isolates_input_then_keeps_only_accepted_geometry(cx: &mut TestAppContext) {
    let Fixture {
        mut visual,
        editor,
        actions,
        down,
        end,
        _environment,
    } = open(cx);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    super::super::creation_tests::double_down(&mut visual, down, Modifiers::default());
    assert!(editor.read_with(&visual, |v, _| v.midi_creation.is_some()));
    let outside = point(px(640.), px(10.));
    visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_down(outside, MouseButton::Middle, Modifiers::default());
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: outside,
        delta: gpui::ScrollDelta::Lines(point(0., 1.)),
        ..Default::default()
    });
    assert_eq!(actions.get(), 0, "creation owns input before movement");
    visual.simulate_mouse_move(
        point(end.x, outside.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(CursorStyle::ResizeRight)
    );
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    repaint(&editor, &mut visual);
    let held = bodies(&editor, &mut visual);
    assert_eq!(held.len(), 1);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Middle, Modifiers::default());
    let event = terminal(&editor, &mut visual);
    assert!(event.error.is_none());
    for _ in 0..3 {
        repaint(&editor, &mut visual);
        assert_eq!(bodies(&editor, &mut visual), held);
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        editor.read_with(&visual, |v, _| {
            assert!(v.midi_creation.is_none());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
    assert_eq!(actions.get(), 0);
    visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
    assert_eq!(actions.get(), 1);
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    repaint(&editor, &mut visual);
    assert_eq!(bodies(&editor, &mut visual), held);
    editor.read_with(&visual, |v, _| assert!(v.pending_midi_drag.is_none()));
}
#[gpui::test]
fn creation_loss_never_adds_a_note_or_leaks_release(cx: &mut TestAppContext) {
    for loss in [
        "button",
        "escape",
        "focus",
        "invalid",
        "generation",
        "source",
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
        } = open(cx);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        super::super::creation_tests::double_down(&mut visual, down, Modifiers::default());
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
            "source" => editor.update(&mut visual, |v, cx| {
                v.mode = crate::editor::EditorMode::Session;
                v.selected_clip = None;
                v.selected_cell = None;
                cx.notify();
            }),
            "hidden" => editor.update(&mut visual, |v, cx| {
                v.clip_view_open = false;
                cx.notify();
            }),
            "bounds" => visual.simulate_resize(gpui::size(px(1000.), px(700.))),
            "window" => visual.deactivate_window(),
            "outside" => visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default()),
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
        assert_eq!(actions.get(), 0, "{loss}: no trailing release");
        editor.read_with(&visual, |v, _| {
            assert!(v.midi_creation.is_none(), "{loss}");
            assert!(v.pending_midi_drag.is_none());
            assert!(!v.edit_pending(), "{loss}");
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                before,
                "{loss}"
            );
        });
    }
}

#[gpui::test]
fn linked_alias_creation_keeps_receipt_geometry_and_shared_one_step_history(
    cx: &mut TestAppContext,
) {
    let Fixture {
        mut visual,
        editor,
        _environment,
        ..
    } = open(cx);
    let original = editor.read_with(&visual, |v, _| v.detail_clip_id().unwrap());
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::DuplicateAliasClips(vec![original]))));
    });
    pump(&editor, &mut visual);
    let alias = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        let alias = p.tracks()[0]
            .midi_clips()
            .iter()
            .find(|c| c.id() != original)
            .unwrap()
            .id();
        assert!(p.clip_alias(original).is_some());
        assert_eq!(p.clip_alias(original), p.clip_alias(alias));
        alias
    });
    visual.update(|w, cx| {
        editor.update(cx, |v, cx| {
            v.selected_clip = Some(alias);
            v.piano_view_clip = None;
            v.midi_focus.focus(w);
            cx.notify();
        })
    });
    repaint(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let down = position(&editor, &mut visual, 1., 60);
    let end = position(&editor, &mut visual, 2., 60);
    super::super::creation_tests::double_down(&mut visual, down, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    repaint(&editor, &mut visual);
    let held = bodies(&editor, &mut visual);
    assert_eq!(held.len(), 1);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    let reply = terminal(&editor, &mut visual);
    assert!(reply.error.is_none());
    for _ in 0..3 {
        repaint(&editor, &mut visual);
        assert_eq!(bodies(&editor, &mut visual), held);
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(reply);
        cx.notify();
    });
    repaint(&editor, &mut visual);
    assert_eq!(bodies(&editor, &mut visual), held);
    let after = editor.read_with(&visual, |v, _| {
        assert!(v.pending_midi_drag.is_none());
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(p.midi_source(alias).unwrap().notes().len(), 1);
        assert_eq!(
            p.midi_source(alias).unwrap().notes(),
            p.midi_source(original).unwrap().notes()
        );
        p.snapshot()
    });
    for (request, expected) in [(Request::Undo, before), (Request::Redo, after)] {
        editor.update(&mut visual, |v, _| assert!(v.enqueue(request)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let actual = v.state.as_ref().unwrap().project.snapshot();
            let mut expected = expected;
            assert!(actual.next_id >= expected.next_id);
            expected.next_id = actual.next_id;
            assert_eq!(actual, expected);
        });
    }
    assert_eq!(bodies(&editor, &mut visual), held);
}
