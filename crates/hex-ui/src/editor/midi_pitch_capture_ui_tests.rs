//! Actual key-gutter input with local viewport custody and no musical edit.
use super::*;
use crate::editor::EditorView;
use gpui::{
    CursorStyle, Entity, IntoElement, Modifiers, MouseButton, MouseMoveEvent, MouseUpEvent,
    PlatformInput, Render, VisualTestContext, div, prelude::*,
};
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
fn repaint(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    editor.update(visual, |_, cx| cx.notify());
    visual.run_until_parked();
}
fn mount(editor: &Entity<EditorView>, visual: &mut VisualTestContext) -> Rc<Cell<usize>> {
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
fn key_zoom_owns_input_and_uses_outside_release_without_project_edit(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    let actions = mount(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let down = visual.debug_bounds("midi-piano-keys").unwrap().center();
    let outside = point(px(640.), px(10.));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
    visual.simulate_event(ScrollWheelEvent {
        position: outside,
        delta: ScrollDelta::Lines(point(0., 1.)),
        ..Default::default()
    });
    assert_eq!(actions.get(), 0, "exclusive before motion");
    visual.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
    for _ in 0..3 {
        repaint(&editor, &mut visual);
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(CursorStyle::ResizeUpDown)
        );
    }
    // Actual final coordinate differs from the preceding move and lies outside the gutter.
    visual.simulate_mouse_up(
        point(outside.x, down.y - px(120.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
    repaint(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.piano_pitch_zoom.is_none());
        assert_eq!((v.piano_pitch_low, v.piano_pitch_rows), (54, 12));
        assert!(!v.edit_pending());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    assert_eq!(actions.get(), 0);
    assert!(!visual.update(|w, _| w.is_pointer_dragging()));
    visual.simulate_event(ScrollWheelEvent {
        position: outside,
        delta: ScrollDelta::Lines(point(0., 1.)),
        ..Default::default()
    });
    assert_eq!(actions.get(), 1, "input free after release");
}
#[gpui::test]
fn key_zoom_loss_keeps_last_view_and_suppresses_trailing_release(cx: &mut TestAppContext) {
    for loss in [
        "button",
        "escape",
        "focus",
        "invalid",
        "source",
        "generation",
        "revision",
        "bounds",
        "hidden",
        "device",
        "window",
    ] {
        let cx = &mut cx.new_app();
        let (mut visual, editor, _environment, _clip) = fixture(cx);
        let actions = mount(&editor, &mut visual);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let down = visual.debug_bounds("midi-piano-keys").unwrap().center();
        let end = down - point(px(0.), px(120.));
        let outside = point(px(640.), px(10.));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        repaint(&editor, &mut visual);
        assert_eq!(
            editor.read_with(&visual, |v, _| (v.piano_pitch_low, v.piano_pitch_rows)),
            (54, 12)
        );
        match loss {
            "button" => visual.simulate_mouse_move(outside, None, Modifiers::default()),
            "escape" => visual.simulate_keystrokes("escape"),
            "focus" | "invalid" => visual.update(|w, cx| {
                if loss == "focus" {
                    editor.update(cx, |v, _| v.focus.focus(w));
                } else {
                    w.dispatch_input(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position: point(px(f32::NAN), end.y),
                            pressed_button: Some(MouseButton::Left),
                            ..Default::default()
                        }),
                        cx,
                    );
                }
                w.dispatch_input(
                    PlatformInput::MouseUp(MouseUpEvent {
                        position: outside,
                        button: MouseButton::Left,
                        ..Default::default()
                    }),
                    cx,
                );
            }),
            "source" => editor.update(&mut visual, |v, cx| {
                v.selected_clip = None;
                cx.notify();
            }),
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
            "bounds" => visual.simulate_resize(gpui::size(px(1400.), px(900.))),
            "hidden" => editor.update(&mut visual, |v, cx| {
                v.clip_view_open = false;
                cx.notify();
            }),
            "device" => editor.update(&mut visual, |v, cx| {
                v.device_panel_active = true;
                cx.notify();
            }),
            "window" => visual.deactivate_window(),
            _ => unreachable!(),
        }
        repaint(&editor, &mut visual);
        if loss != "focus" && loss != "invalid" {
            visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        }
        assert_eq!(actions.get(), 0, "{loss}: trailing up");
        editor.read_with(&visual, |v, _| {
            assert!(v.piano_pitch_zoom.is_none(), "{loss}");
            assert_eq!((v.piano_pitch_low, v.piano_pitch_rows), (54, 12), "{loss}");
            assert!(!v.edit_pending());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
}
