//! Actual held draw/erase input, with accepted display left to its receipt.
use super::*;
use gpui::{
    CursorStyle, MouseMoveEvent, MouseUpEvent, Render, ScrollDelta, ScrollWheelEvent, div,
    prelude::*,
};
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
fn open(
    cx: &mut TestAppContext,
    erase: bool,
) -> (
    VisualTestContext,
    Entity<EditorView>,
    Arc<crate::editor::test_environment::Environment>,
    Rc<Cell<usize>>,
) {
    let (mut visual, editor, environment, clip) = fixture(cx);
    if erase {
        editor.update(&mut visual, |v, _| {
            assert!(
                v.enqueue(Request::Edit(Command::AddMidiNotes {
                    clip,
                    notes: [0.5, 1.0]
                        .into_iter()
                        .map(|start| MidiNoteInsert {
                            active: true,
                            start: beat(start),
                            duration: beat(0.5),
                            pitch: 60,
                            channel: 0,
                            velocity: 93
                        })
                        .collect()
                }))
            )
        });
        pump(&editor, &mut visual);
    }
    configure(&editor, &mut visual, 4);
    editor.update(&mut visual, |v, cx| {
        v.midi_draw_pitch_locked = true;
        cx.notify();
    });
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
        });
    });
    repaint(&editor, &mut visual);
    (visual, editor, environment, actions)
}
#[gpui::test]
fn draw_and_erase_own_input_then_handoff_preview_without_holding_capture(cx: &mut TestAppContext) {
    for erase in [false, true] {
        let cx = &mut cx.new_app();
        let (mut visual, editor, _environment, actions) = open(cx, erase);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let down = position(&editor, &mut visual, if erase { 0.61 } else { 0.51 }, 60);
        let end = position(&editor, &mut visual, if erase { 1.11 } else { 1.01 }, 60);
        let outside = point(end.x, px(10.));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        assert!(editor.read_with(&visual, |v, _| v.midi_stroke.is_some()));
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        visual.simulate_event(ScrollWheelEvent {
            position: outside,
            delta: ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(actions.get(), 0, "erase={erase}: exclusive before motion");
        visual.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
        repaint(&editor, &mut visual);
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        repaint(&editor, &mut visual);
        let held = geometry(&editor, &mut visual);
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
        let reply = terminal(&editor, &mut visual);
        assert!(reply.error.is_none());
        for _ in 0..3 {
            repaint(&editor, &mut visual);
            assert_eq!(geometry(&editor, &mut visual), held);
            assert!(!visual.update(|w, _| w.is_pointer_dragging()));
            editor.read_with(&visual, |v, _| {
                assert!(v.midi_stroke.is_none());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
        assert_eq!(actions.get(), 0);
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        assert_eq!(actions.get(), 1);
        editor.update(&mut visual, |v, cx| {
            v.receive(reply);
            cx.notify();
        });
        repaint(&editor, &mut visual);
        assert_eq!(geometry(&editor, &mut visual), held);
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        history(&editor, &visual, &before);
    }
}
#[gpui::test]
fn draw_input_loss_never_submits_a_partial_or_stale_stroke_or_leaks_release(
    cx: &mut TestAppContext,
) {
    for erase in [false, true] {
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
            "window",
            "snap",
            "pitch",
            "mode",
            "outside",
        ] {
            let cx = &mut cx.new_app();
            let (mut visual, editor, _environment, actions) = open(cx, erase);
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            let down = position(&editor, &mut visual, if erase { 0.61 } else { 0.51 }, 60);
            let end = position(&editor, &mut visual, if erase { 1.11 } else { 1.01 }, 60);
            let outside = point(end.x, px(10.));
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
            repaint(&editor, &mut visual);
            match loss {
                "button" => visual.simulate_mouse_move(outside, None, Modifiers::default()),
                "escape" => visual.simulate_keystrokes("escape"),
                "focus" | "invalid" => visual.update(|w, cx| {
                    if loss == "focus" {
                        editor.update(cx, |v, _| v.focus.focus(w));
                    } else {
                        w.dispatch_input(
                            gpui::PlatformInput::MouseMove(MouseMoveEvent {
                                position: point(px(f32::NAN), end.y),
                                pressed_button: Some(MouseButton::Left),
                                ..Default::default()
                            }),
                            cx,
                        );
                    }
                    w.dispatch_input(
                        gpui::PlatformInput::MouseUp(MouseUpEvent {
                            position: end,
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
                "bounds" => visual.simulate_resize(gpui::size(px(1400.), px(900.))),
                "hidden" => editor.update(&mut visual, |v, cx| {
                    v.clip_view_open = false;
                    cx.notify();
                }),
                "window" => visual.deactivate_window(),
                "snap" => visual.simulate_modifiers_change(Modifiers {
                    platform: true,
                    ..Default::default()
                }),
                "pitch" => visual.simulate_modifiers_change(Modifiers {
                    alt: true,
                    ..Default::default()
                }),
                "mode" => visual.simulate_keystrokes("b"),
                "outside" => {
                    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default())
                }
                _ => unreachable!(),
            }
            repaint(&editor, &mut visual);
            if !matches!(loss, "focus" | "invalid" | "outside") {
                visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
            }
            assert_eq!(actions.get(), 0, "erase={erase}/{loss}: trailing release");
            editor.read_with(&visual, |v, _| {
                assert!(v.midi_stroke.is_none(), "erase={erase}/{loss}");
                assert!(v.pending_midi_drag.is_none());
                assert!(!v.edit_pending());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
    }
}
#[gpui::test]
#[ignore = "opt-in held/released draw dispatch and repaint CPU probe"]
fn draw_dispatch_repaint_latency(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _actions) = open(cx, false);
    let down = position(&editor, &mut visual, 0.51, 60);
    let end = position(&editor, &mut visual, 1.01, 60);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    for phase in ["held", "released"] {
        if phase == "released" {
            visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
            let reply = terminal(&editor, &mut visual);
            assert!(reply.error.is_none());
        }
        let mut samples = Vec::new();
        for n in 0..144 {
            let start = std::time::Instant::now();
            if phase == "held" {
                visual.simulate_mouse_move(
                    end + point(px((n % 8) as f32), px(0.)),
                    Some(MouseButton::Left),
                    Modifiers::default(),
                );
            }
            repaint(&editor, &mut visual);
            if n >= 16 {
                samples.push(start.elapsed().as_secs_f64() * 1_000_000.);
            }
        }
        samples.sort_by(f64::total_cmp);
        let bodies = geometry(&editor, &mut visual).len();
        assert!(bodies > 0);
        println!(
            "draw_repaint_us phase={phase} p50={} p95={} max={} bodies={bodies} tracks=1 clips=1 canonical_notes=0 warmups=16 samples=128",
            samples[64], samples[121], samples[127]
        );
    }
}
