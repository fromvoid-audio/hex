//! Both marquee surfaces share live selection but own input only while held.
use super::*;
fn range(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
    velocity: bool,
) -> (gpui::Point<gpui::Pixels>, gpui::Point<gpui::Pixels>) {
    if velocity {
        let b = visual.debug_bounds("midi-velocity-lane").unwrap();
        (
            b.origin + point(b.size.width * (0.85 / 4.), px(2.)),
            b.origin + point(b.size.width * (1.15 / 4.), px(54.)),
        )
    } else {
        let b = visual.debug_bounds("midi-note-roll").unwrap();
        editor.read_with(visual, |v, _| {
            let origin = b.origin + point(px(1.), px(1.));
            let width = b.size.width - px(2.);
            let height = b.size.height - px(2.);
            let row = height / f32::from(v.piano_pitch_rows);
            let top = row * f32::from(v.piano_pitch_low + v.piano_pitch_rows - 1 - 64);
            (
                origin + point(width * (0.85 / 4.), top - px(1.)),
                origin + point(width * (1.15 / 4.), top + row + px(1.)),
            )
        })
    }
}
fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Default::default()
    }
}
fn held(v: &EditorView, velocity: bool) -> bool {
    if velocity {
        v.midi_velocity.marquee.is_some()
    } else {
        v.midi_marquee.is_some()
    }
}
fn selected_handles(visual: &mut VisualTestContext) -> usize {
    let scale = gpui::ScaledPixels::from(visual.update(|w, _| w.scale_factor()));
    let primary: gpui::Background = rgb(theme::color(theme::PaletteRole::Primary)).into();
    visual
        .rendered_filled_quads("midi-velocity-lane")
        .iter()
        .filter(|(b, c)| {
            c == &primary
                && (b.size.height / scale - 4.).abs() < 0.01
                && (b.size.width / scale - 6.).abs() < 0.01
        })
        .count()
}
#[gpui::test]
fn marquees_own_input_and_keep_live_selection_through_completed_paints_and_release(
    cx: &mut TestAppContext,
) {
    for velocity in [false, true] {
        let cx = &mut cx.new_app();
        let (mut visual, editor, _environment, _clip) = setup(cx);
        let actions = host(&mut visual, &editor, false);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let baseline = editor.read_with(&visual, |v, _| v.midi_selection.ids().to_vec());
        let (down, end) = range(&editor, &mut visual, velocity);
        let outside = point(px(640.), px(10.));
        visual.simulate_mouse_down(down, MouseButton::Left, shift());
        assert!(editor.read_with(&visual, |v, _| held(v, velocity)));
        visual.simulate_mouse_down(outside, MouseButton::Right, shift());
        visual.simulate_event(ScrollWheelEvent {
            position: outside,
            delta: ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(
            actions.get(),
            0,
            "velocity={velocity}: exclusive before motion"
        );
        visual.simulate_mouse_move(outside, Some(MouseButton::Left), shift());
        repaint(&editor, &mut visual);
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
        visual.simulate_mouse_move(end, Some(MouseButton::Left), shift());
        for _ in 0..3 {
            repaint(&editor, &mut visual);
            let n = editor.read_with(&visual, |v, _| {
                assert!(held(v, velocity));
                assert!(v.midi_selection.contains(baseline[0]));
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
                v.midi_selection.len()
            });
            assert_eq!(n, if velocity { 3 } else { 2 });
            assert_eq!(selected_handles(&mut visual), n);
        }
        let selected = editor.read_with(&visual, |v, _| v.midi_selection.ids().to_vec());
        visual.simulate_mouse_up(end, MouseButton::Left, shift());
        visual.simulate_mouse_up(outside, MouseButton::Right, shift());
        for _ in 0..3 {
            repaint(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert!(!held(v, velocity));
                assert_eq!(v.midi_selection.ids(), selected);
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
            assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        }
        assert_eq!(actions.get(), 0);
        visual.simulate_mouse_down(outside, MouseButton::Right, shift());
        assert_eq!(actions.get(), 1);
    }
}
#[gpui::test]
fn marquee_loss_restores_owned_baseline_but_preserves_newer_selection(cx: &mut TestAppContext) {
    for velocity in [false, true] {
        for loss in [
            "button",
            "escape",
            "focus",
            "invalid",
            "newer-selection",
            "generation",
            "source",
            "viewport",
            "bounds",
            "hidden",
            "collapsed",
            "window",
        ] {
            if loss == "collapsed" && !velocity {
                continue;
            }
            let cx = &mut cx.new_app();
            let (mut visual, editor, _environment, clip) = setup(cx);
            let actions = host(&mut visual, &editor, false);
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            let baseline = editor.read_with(&visual, |v, _| v.midi_selection.ids().to_vec());
            let newer = editor.read_with(&visual, |v, _| {
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .midi_source(clip)
                    .unwrap()
                    .notes()
                    .iter()
                    .find(|n| n.pitch() == 0)
                    .unwrap()
                    .id()
            });
            let (down, end) = range(&editor, &mut visual, velocity);
            let outside = point(px(640.), px(10.));
            visual.simulate_mouse_down(down, MouseButton::Left, shift());
            visual.simulate_mouse_move(end, Some(MouseButton::Left), shift());
            repaint(&editor, &mut visual);
            assert!(editor.read_with(&visual, |v, _| v.midi_selection.len() > baseline.len()));
            match loss {
                "button" => visual.simulate_mouse_move(outside, None, shift()),
                "escape" => visual.simulate_keystrokes("escape"),
                "focus" | "invalid" => visual.update(|w, cx| {
                    if loss == "focus" {
                        editor.update(cx, |v, _| v.focus.focus(w));
                    } else {
                        w.dispatch_input(
                            gpui::PlatformInput::MouseMove(MouseMoveEvent {
                                position: point(end.x, px(f32::NAN)),
                                pressed_button: Some(MouseButton::Left),
                                modifiers: shift(),
                            }),
                            cx,
                        );
                    }
                    w.dispatch_input(
                        gpui::PlatformInput::MouseUp(MouseUpEvent {
                            position: outside,
                            button: MouseButton::Left,
                            modifiers: shift(),
                            click_count: 1,
                        }),
                        cx,
                    );
                }),
                "newer-selection" => editor.update(&mut visual, |v, cx| {
                    v.midi_selection.set_single(newer);
                    cx.notify();
                }),
                "generation" => editor.update(&mut visual, |v, cx| {
                    v.project_generation += 1;
                    v.midi_selection.set_single(newer);
                    cx.notify();
                }),
                "source" => editor.update(&mut visual, |v, cx| {
                    v.selected_clip = None;
                    v.midi_selection.set_single(newer);
                    cx.notify();
                }),
                "viewport" => editor.update(&mut visual, |v, cx| {
                    v.piano_beat_span = BeatTime::from_quarters(8).unwrap();
                    cx.notify();
                }),
                "bounds" => visual.simulate_resize(gpui::size(px(1400.), px(900.))),
                "collapsed" => editor.update(&mut visual, |v, cx| {
                    v.midi_velocity.open = false;
                    cx.notify();
                }),
                "hidden" => editor.update(&mut visual, |v, cx| {
                    v.clip_view_open = false;
                    cx.notify();
                }),
                "window" => visual.deactivate_window(),
                _ => unreachable!(),
            }
            repaint(&editor, &mut visual);
            if loss != "focus" && loss != "invalid" {
                visual.simulate_mouse_up(outside, MouseButton::Left, shift());
            }
            assert_eq!(
                actions.get(),
                0,
                "velocity={velocity}/{loss}: trailing release"
            );
            editor.read_with(&visual, |v, _| {
                assert!(!held(v, velocity), "velocity={velocity}/{loss}");
                if matches!(loss, "newer-selection" | "generation" | "source") {
                    assert_eq!(v.midi_selection.ids(), [newer]);
                } else {
                    assert_eq!(v.midi_selection.ids(), baseline);
                }
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
    }
}
#[gpui::test]
#[ignore = "opt-in both-surface marquee dispatch and repaint CPU probe"]
fn marquee_dispatch_repaint_latency(cx: &mut TestAppContext) {
    for velocity in [false, true] {
        let cx = &mut cx.new_app();
        let (mut visual, editor, _environment, _clip) = setup(cx);
        let _actions = host(&mut visual, &editor, false);
        let (down, end) = range(&editor, &mut visual, velocity);
        visual.simulate_mouse_down(down, MouseButton::Left, shift());
        let mut samples = Vec::new();
        for n in 0..144 {
            let start = Instant::now();
            visual.simulate_mouse_move(
                end + point(px((n % 8) as f32), px(0.)),
                Some(MouseButton::Left),
                shift(),
            );
            repaint(&editor, &mut visual);
            if n >= 16 {
                samples.push(start.elapsed().as_secs_f64() * 1_000_000.);
            }
        }
        visual.simulate_mouse_up(end, MouseButton::Left, shift());
        samples.sort_by(f64::total_cmp);
        println!(
            "marquee_repaint_us velocity={velocity} p50={} p95={} max={} tracks=1 clips=1 notes=3 warmups=16 samples=128",
            samples[64], samples[121], samples[127]
        );
    }
}
