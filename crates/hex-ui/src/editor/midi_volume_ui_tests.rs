//! Actual shared graph pointer capture and released preview through Service.
use super::super::midi_canvas::creation_tests::{fixture, pump};
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, point};
use hex_session::service::Request;
use std::time::{Duration, Instant};

#[gpui::test]
fn graph_creation_release_keeps_preview_until_matching_worker_terminal_and_one_undo(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, cx| {
        v.midi_volume.enabled = true;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("midi-volume-point-values").is_none());
    let bounds = editor.read_with(&visual, |v, cx| {
        v.midi_volume
            .graph
            .as_ref()
            .unwrap()
            .read(cx)
            .plot_bounds()
            .unwrap()
    });
    let down = point(
        bounds.left() + bounds.size.width * 0.25,
        bounds.top() + bounds.size.height * 0.75,
    );
    let end = down + point(bounds.size.width * 0.25, -bounds.size.height * 0.25);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let held = editor.read_with(&visual, |v, _| v.displayed_midi_volume(clip).unwrap());
    assert!(visual.debug_bounds("midi-volume-point-values").is_some());
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            point_values(&held, v.midi_volume.selected).as_deref(),
            Some("beat 2 · gain 2")
        );
    });
    assert_eq!(held.points().len(), 1);
    assert!((held.points()[0].value.linear() - 2.0).abs() < 0.001);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(
                curve(&v.state.as_ref().unwrap().project, clip)
                    .unwrap()
                    .points()
                    .is_empty()
            );
            let displayed = v.displayed_midi_volume(clip).unwrap();
            assert_eq!(displayed, held);
            assert_eq!(
                point_values(&displayed, v.midi_volume.selected).as_deref(),
                Some("beat 2 · gain 2")
            );
            assert!(v.midi_volume.gesture.is_none());
            assert!(v.pending_midi_drag.is_some());
        });
        assert!(visual.debug_bounds("midi-volume-point-values").is_some());
    }
    let until = Instant::now() + Duration::from_secs(5);
    let terminal = loop {
        if let Ok(e) = editor.read_with(&visual, |v, _| v.service.try_event()) {
            if e.request_complete {
                break e;
            }
            editor.update(&mut visual, |v, _| v.receive(e));
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    editor.update(&mut visual, |v, cx| {
        v.receive(terminal);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.displayed_midi_volume(clip).unwrap(), held);
        assert!(v.pending_midi_drag.is_none());
        assert_eq!(
            point_values(
                &v.displayed_midi_volume(clip).unwrap(),
                v.midi_volume.selected
            )
            .as_deref(),
            Some("beat 2 · gain 2")
        );
    });
    assert!(visual.debug_bounds("midi-volume-point-values").is_some());
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Undo));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.displayed_midi_volume(clip).unwrap().points().is_empty())
    });
    assert!(visual.debug_bounds("midi-volume-point-values").is_none());
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Redo));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.displayed_midi_volume(clip).unwrap(), held)
    });
    let saved = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let path = _environment.root().join("clip-volume.hex");
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::SaveNew(path.clone())))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Open(path))));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), saved)
    });
}

#[gpui::test]
fn envelope_escape_blur_lost_pointer_and_stale_source_cancel_without_history(
    cx: &mut TestAppContext,
) {
    for reason in 0..4 {
        let (mut visual, editor, _environment, clip) = fixture(cx);
        editor.update(&mut visual, |v, cx| {
            v.midi_volume.enabled = true;
            cx.notify();
        });
        visual.run_until_parked();
        let bounds = editor.read_with(&visual, |v, cx| {
            v.midi_volume
                .graph
                .as_ref()
                .unwrap()
                .read(cx)
                .plot_bounds()
                .unwrap()
        });
        let down = point(
            bounds.left() + bounds.size.width * 0.25,
            bounds.top() + bounds.size.height * 0.75,
        );
        let end = down + point(bounds.size.width * 0.25, -bounds.size.height * 0.25);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        assert!(editor.read_with(&visual, |v, _| v.midi_volume.gesture.is_some()));
        assert!(visual.debug_bounds("midi-volume-point-values").is_some());
        // A/0 belongs to contextual clip activation only outside nested plot
        // focus. It must not submit an edit underneath a held envelope node.
        visual.simulate_keystrokes("a");
        visual.simulate_keystrokes("0");
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(v.midi_volume.gesture.is_some());
            assert!(!v.edit_pending());
            assert_eq!(
                v.state.as_ref().unwrap().project.clip_active(clip),
                Some(true)
            );
        });
        match reason {
            0 => visual.simulate_keystrokes("escape"),
            1 => visual.update(|window, _| window.blur()),
            2 => visual.simulate_mouse_move(end, None, Modifiers::default()),
            _ => {
                editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
                        clip,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch: 60,
                        channel: 0,
                        velocity: 100,
                    })));
                });
                pump(&editor, &mut visual);
            }
        }
        visual.run_until_parked();
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.midi_volume.gesture.is_none(), "reason {reason}");
            assert!(v.pending_midi_drag.is_none());
            assert!(
                point_values(
                    &v.displayed_midi_volume(clip).unwrap(),
                    v.midi_volume.selected
                )
                .is_none()
            );
            assert!(
                curve(&v.state.as_ref().unwrap().project, clip)
                    .unwrap()
                    .points()
                    .is_empty()
            );
        });
        assert!(visual.debug_bounds("midi-volume-point-values").is_none());
    }
}
