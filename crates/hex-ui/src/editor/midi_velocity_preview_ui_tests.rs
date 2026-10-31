//! Completed paints with the real worker terminal deliberately withheld.
use super::*;
fn handles(visual: &mut VisualTestContext) -> Vec<[i32; 4]> {
    let scale = gpui::ScaledPixels::from(visual.update(|w, _| w.scale_factor()));
    let mut result: Vec<_> = visual
        .rendered_filled_quads("midi-velocity-lane")
        .into_iter()
        .filter(|(b, _)| {
            (b.size.height / scale - 4.).abs() < 0.01 && (b.size.width / scale - 6.).abs() < 0.01
        })
        .map(|(b, _)| {
            [
                b.origin.x / scale,
                b.origin.y / scale,
                b.size.width / scale,
                b.size.height / scale,
            ]
            .map(|x| (x * 100.).round() as i32)
        })
        .collect();
    result.sort_unstable();
    result
}
fn repaint(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    editor.update(&mut *visual, |_, cx| cx.notify());
    visual.run_until_parked();
}
fn terminal(editor: &Entity<EditorView>, visual: &mut VisualTestContext) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = editor.read_with(&*visual, |v, _| v.service.try_event()) {
            if event.request_complete {
                return event;
            }
            editor.update(&mut *visual, |v, cx| {
                v.receive(event);
                cx.notify();
            });
        }
        assert!(Instant::now() < deadline, "real velocity terminal");
        std::thread::yield_now();
    }
}
#[gpui::test]
fn released_velocity_handles_keep_final_values_until_exact_real_terminal(cx: &mut TestAppContext) {
    for group in [false, true] {
        let cx = &mut cx.new_app();
        let (mut visual, editor, _environment, clip) = setup(cx);
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
        repaint(&editor, &mut visual);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let baseline = handles(&mut visual);
        assert_eq!(baseline.len(), 3);
        let down = handle(&mut visual, 0.5, 37);
        let end = down - point(px(0.), px(14.));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        repaint(&editor, &mut visual);
        let held = handles(&mut visual);
        assert_ne!(held, baseline);
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        let event = terminal(&editor, &mut visual);
        assert!(event.error.is_none());
        for _ in 0..3 {
            repaint(&editor, &mut visual);
            assert_eq!(
                handles(&mut visual),
                held,
                "released preview must survive completed paints"
            );
            editor.read_with(&visual, |v, _| {
                assert!(v.midi_velocity.gesture.is_none());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
        // Selection/focus are no longer input custody after release.
        visual.update(|w, cx| {
            editor.update(cx, |v, cx| {
                v.midi_selection.clear();
                v.focus.focus(w);
                cx.notify();
            })
        });
        repaint(&editor, &mut visual);
        assert_eq!(handles(&mut visual), held);
        editor.update(&mut visual, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        repaint(&editor, &mut visual);
        assert_eq!(handles(&mut visual), held);
        editor.read_with(&visual, |v, _| assert!(v.pending_midi_drag.is_none()));
        let after = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        for (command, expected) in [(Request::Undo, &before), (Request::Redo, &after)] {
            editor.update(&mut visual, |v, _| assert!(v.enqueue(command)));
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(&v.state.as_ref().unwrap().project.snapshot(), expected)
            });
        }
    }
}

#[path = "midi_velocity_capture_ui_tests.rs"]
mod capture_tests;
