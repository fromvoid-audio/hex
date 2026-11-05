//! Released draw geometry with actual Service terminals deliberately withheld.
use super::*;
fn geometry(editor: &Entity<EditorView>, visual: &mut VisualTestContext) -> Vec<[i32; 4]> {
    let b = visual.debug_bounds("midi-note-roll").unwrap();
    let rows = editor.read_with(&*visual, |v, _| v.piano_pitch_rows);
    let height = ((f32::from(b.size.height) - 2.) / f32::from(rows) - 2.).max(1.);
    let scale = gpui::ScaledPixels::from(visual.update(|w, _| w.scale_factor()));
    let mut result: Vec<_> = visual
        .rendered_filled_quads("midi-note-roll")
        .into_iter()
        // Canonical notes also paint one-pixel leading/trailing borders.
        .filter(|(b, _)| {
            (b.size.height / scale - height).abs() < 0.01 && b.size.width / scale > 1.01
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
fn configure(editor: &Entity<EditorView>, visual: &mut VisualTestContext, span: u64) {
    editor.update(&mut *visual, |v, cx| {
        v.midi_draw_mode = true;
        v.midi_snap_grid = super::super::midi_drag::MidiGrid::Sixteenth;
        v.piano_beat_span = BeatTime::from_quarters(span).unwrap();
        v.midi_selection.clear();
        cx.notify();
    });
    repaint(editor, visual);
}
fn history(
    editor: &Entity<EditorView>,
    visual: &VisualTestContext,
    expected: &hex_project::Snapshot,
) {
    editor.read_with(visual, |v, _| {
        let actual = v.state.as_ref().unwrap().project.snapshot();
        let mut expected = expected.clone();
        assert!(actual.next_id >= expected.next_id);
        expected.next_id = actual.next_id;
        assert_eq!(actual, expected);
    });
}
#[gpui::test]
fn released_draw_erase_overwrite_and_extension_keep_geometry_until_real_terminal(
    cx: &mut TestAppContext,
) {
    for case in ["draw", "erase", "overwrite", "extend"] {
        let cx = &mut cx.new_app();
        let (mut visual, editor, _environment, clip) = fixture(cx);
        if case == "erase" || case == "overwrite" {
            let notes: Vec<_> = if case == "erase" {
                vec![(0.5, 0.5), (1.0, 0.5)]
            } else {
                vec![(0.625, 1.0)]
            }
            .into_iter()
            .map(|(start, duration)| MidiNoteInsert {
                active: true,
                start: beat(start),
                duration: beat(duration),
                pitch: 60,
                channel: 0,
                velocity: 93,
            })
            .collect();
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::AddMidiNotes { clip, notes })))
            });
            pump(&editor, &mut visual);
        }
        configure(&editor, &mut visual, if case == "extend" { 8 } else { 4 });
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let baseline = geometry(&editor, &mut visual);
        let (start, end) = match case {
            "erase" => (0.61, 1.11),
            "overwrite" => (0.51, 0.52),
            "extend" => (4.51, 5.01),
            _ => (0.51, 1.01),
        };
        let down = position(&editor, &mut visual, start, 60);
        let end = position(&editor, &mut visual, end, 60);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        repaint(&editor, &mut visual);
        let held = geometry(&editor, &mut visual);
        assert_ne!(held, baseline, "{case}: meaningful preview");
        if case == "erase" {
            assert!(held.is_empty());
        } else {
            assert!(!held.is_empty());
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        let reply = terminal(&editor, &mut visual);
        assert!(reply.error.is_none(), "{case}: {:?}", reply.error);
        for _ in 0..3 {
            repaint(&editor, &mut visual);
            assert_eq!(
                geometry(&editor, &mut visual),
                held,
                "{case}: released preview"
            );
            editor.read_with(&visual, |v, _| {
                assert!(v.midi_stroke.is_none());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
        editor.update(&mut visual, |v, cx| {
            v.receive(reply);
            cx.notify();
        });
        repaint(&editor, &mut visual);
        assert_eq!(
            geometry(&editor, &mut visual),
            held,
            "{case}: canonical installation"
        );
        let after = editor.read_with(&visual, |v, _| {
            assert!(v.pending_midi_drag.is_none());
            if case == "extend" {
                assert!(
                    v.state
                        .as_ref()
                        .unwrap()
                        .project
                        .midi_source(clip)
                        .unwrap()
                        .source_length()
                        > BeatTime::from_quarters(4).unwrap()
                );
            }
            v.state.as_ref().unwrap().project.snapshot()
        });
        for (command, expected) in [(Request::Undo, &before), (Request::Redo, &after)] {
            editor.update(&mut visual, |v, _| assert!(v.enqueue(command)));
            pump(&editor, &mut visual);
            history(&editor, &visual, expected);
        }
    }
}
#[gpui::test]
fn queued_draw_batch_matches_only_its_exact_variant_and_survives_earlier_terminal(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    configure(&editor, &mut visual, 4);
    let down = position(&editor, &mut visual, 0.51, 60);
    let end = position(&editor, &mut visual, 1.01, 60);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    repaint(&editor, &mut visual);
    let held = geometry(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        let tuning = v.state.as_ref().unwrap().project.tuning();
        v.tuning.client.begin(v.project_generation, tuning).unwrap();
        v.tuning.client.change(tuning.with_enabled(true)).unwrap();
        v.tuning.client.finish(tuning.with_enabled(true)).unwrap();
    });
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    repaint(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.pending_requests.len(), 1);
        let command = v.last_enqueued_edit.clone().unwrap();
        assert!(v.pending_midi_drag_matches(&Request::Edit(command.clone())));
        let Command::EditMidiNotes {
            clip: c,
            remove,
            mut insert,
        } = command
        else {
            panic!("ordinary draw batch");
        };
        assert_eq!(c, clip);
        assert!(!v.pending_midi_drag_matches(&Request::Edit(
            Command::EditMidiNotesWithSourceExtent {
                clip: c,
                remove: remove.clone(),
                insert: insert.clone()
            }
        )));
        insert[0].velocity -= 1;
        assert!(
            !v.pending_midi_drag_matches(&Request::Edit(Command::EditMidiNotes {
                clip: c,
                remove,
                insert
            }))
        );
    });
    // The real controller drains the earlier tuning terminal before submitting
    // the queued draw at the end of tick. The draw terminal remains unreceived.
    let deadline = crate::test_wait::hang_deadline();
    while editor.read_with(&visual, |v, _| !v.pending_requests.is_empty()) {
        editor.update(&mut visual, |v, cx| {
            v.tick(cx);
            cx.notify();
        });
        repaint(&editor, &mut visual);
        assert_eq!(geometry(&editor, &mut visual), held);
        assert!(
            std::time::Instant::now() < deadline,
            "queued_draw_batch_matches_only_its_exact_variant_and_survives_earlier_terminal: hang guard expired"
        );
        std::thread::yield_now();
    }
    let reply = terminal(&editor, &mut visual);
    assert!(reply.error.is_none());
    let before_draw =
        editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    for _ in 0..3 {
        repaint(&editor, &mut visual);
        assert_eq!(geometry(&editor, &mut visual), held);
        editor.read_with(&visual, |v, _| assert!(v.pending_midi_drag.is_some()));
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(reply);
        cx.notify();
    });
    repaint(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        assert!(v.pending_midi_drag.is_none());
        v.state.as_ref().unwrap().project.snapshot()
    });
    assert_eq!(geometry(&editor, &mut visual), held);
    for (command, expected) in [(Request::Undo, &before_draw), (Request::Redo, &after)] {
        editor.update(&mut visual, |v, _| assert!(v.enqueue(command)));
        pump(&editor, &mut visual);
        history(&editor, &visual, expected);
    }
}

#[path = "midi_draw_capture_ui_tests.rs"]
mod capture_tests;
