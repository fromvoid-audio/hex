//! Real signed-view pointer movement, completed repaints and exact worker custody.
use super::{bodies, fixture, position, pump, repaint, terminal};
use gpui::{Modifiers, MouseButton, TestAppContext, point, px};
use hex_project::{BeatTime, Command, MidiNoteInsert};
use hex_session::service::Request;

fn grid_lines(visual: &mut gpui::VisualTestContext) -> Vec<(i32, usize)> {
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
    let colors: [gpui::Background; 2] = [
        gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Grid)).into(),
        gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Border)).into(),
    ];
    let mut lines = visual
        .rendered_filled_quads("midi-note-roll")
        .into_iter()
        .filter(|(b, c)| {
            colors.contains(c)
                && (b.size.width / scale - 1.0).abs() < 0.01
                && (b.size.height / scale - (f32::from(roll.size.height) - 2.0)).abs() < 0.01
        })
        .map(|(b, color)| {
            (
                ((b.origin.x / scale) * 100.0).round() as i32,
                colors.iter().position(|c| c == &color).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    lines.sort_unstable();
    assert!(!lines.is_empty());
    lines
}

#[gpui::test]
fn negative_moves_keep_released_geometry_and_translate_origin_only_on_success(
    cx: &mut TestAppContext,
) {
    for (group, looping) in [(false, false), (true, false), (false, true), (true, true)] {
        let (mut visual, editor, _environment, clip) = fixture(cx);
        let beat = |n, d| BeatTime::from_ratio(n, d).unwrap();
        editor.update(&mut visual, |v, _| {
            assert!(
                v.enqueue(Request::Edit(Command::AddMidiNotes {
                    clip,
                    notes: [
                        (beat(1, 2), beat(1, 1), 60),
                        (BeatTime::ZERO, beat(3, 1), 64),
                        (BeatTime::ZERO, beat(1, 4), 60),
                        (beat(3, 2), beat(1, 2), 67),
                    ]
                    .into_iter()
                    .map(|(start, duration, pitch)| MidiNoteInsert {
                        active: true,
                        start,
                        duration,
                        pitch,
                        channel: 2,
                        velocity: 93
                    })
                    .collect()
                }))
            );
        });
        pump(&editor, &mut visual);
        if looping {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::SetMidiClipSource {
                    clip,
                    source_length: beat(4, 1),
                    source_start: BeatTime::ZERO,
                    loop_start: BeatTime::ZERO,
                    loop_end: beat(4, 1),
                    looping: true,
                    length: beat(4, 1)
                })))
            });
            pump(&editor, &mut visual);
        }
        let (before, moved, held) = editor.update(&mut visual, |v, cx| {
            let project = &v.state.as_ref().unwrap().project;
            let notes = project.midi_source(clip).unwrap().notes();
            let moved = notes.iter().find(|n| n.start() == beat(1, 2)).unwrap().id();
            let held = notes.iter().find(|n| n.pitch() == 64).unwrap().id();
            let before = project.as_ref().clone();
            if group {
                v.midi_selection
                    .select_all(
                        notes
                            .iter()
                            .filter(|n| n.id() == moved || n.pitch() == 67)
                            .map(|n| n.id()),
                    )
                    .unwrap();
            } else {
                v.midi_selection.set_single(moved);
            }
            v.piano_beat_start = -i128::from(beat(1, 1).raw());
            v.midi_marker = Some(beat(1, 1).raw());
            cx.notify();
            (before, moved, held)
        });
        visual.run_until_parked();
        let down = position(&editor, &mut visual, 0.75, 60);
        let roll = visual.debug_bounds("midi-note-roll").unwrap();
        let end = down - point((roll.size.width - px(2.0)) / 4.0, px(0.0));
        let before_bodies = bodies(&editor, &mut visual);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            if group {
                let g = v.midi_group_gesture.as_ref().unwrap();
                assert_eq!(g.source_prefix(), beat(4, 1).raw());
                assert_eq!(g.preview(moved).unwrap().start, beat(7, 2).raw());
            } else {
                let g = v.midi_note_gesture.unwrap();
                assert_eq!(g.source_prefix, beat(4, 1).raw());
                assert_eq!(g.preview.start, beat(7, 2).raw());
            }
            assert_eq!(v.state.as_ref().unwrap().project.as_ref(), &before);
        });
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        repaint(&editor, &mut visual);
        assert_eq!(bodies(&editor, &mut visual), before_bodies);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.as_ref(), &before);
            assert!(!v.edit_pending());
            assert!(v.pending_midi_drag.is_none());
            assert_eq!(v.piano_beat_start, -i128::from(beat(1, 1).raw()));
        });
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        let held_bodies = bodies(&editor, &mut visual);
        let held_grid = grid_lines(&mut visual);
        editor.update(&mut visual, |v, _| {
            let tuning = v.state.as_ref().unwrap().project.tuning();
            v.tuning.client.begin(v.project_generation, tuning).unwrap();
            v.tuning.client.change(tuning.with_enabled(true)).unwrap();
            v.tuning.client.finish(tuning).unwrap();
        });
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        for _ in 0..3 {
            repaint(&editor, &mut visual);
            assert_eq!(bodies(&editor, &mut visual), held_bodies);
        }
        editor.read_with(&visual, |v, _| {
            assert!(v.midi_note_gesture.is_none() && v.midi_group_gesture.is_none());
            assert!(v.pending_midi_drag.is_some());
            let command = v.last_enqueued_edit.clone().unwrap();
            assert!(v.pending_midi_drag_matches(&Request::Edit(command.clone())));
            let Command::UpdateMidiNotesWithSourcePrefix {
                clip,
                prefix,
                notes,
            } = command
            else {
                panic!("prefix command")
            };
            assert!(!v.pending_midi_drag_matches(&Request::Edit(
                Command::UpdateMidiNotesWithSourcePrefix {
                    clip,
                    prefix: BeatTime::from_raw(prefix.raw() + 1),
                    notes
                }
            )));
            assert_eq!(v.piano_beat_start, -i128::from(beat(1, 1).raw()));
        });
        let deadline = crate::test_wait::hang_deadline();
        while editor.read_with(&visual, |v, _| !v.pending_requests.is_empty()) {
            editor.update(&mut visual, |v, cx| {
                v.tick(cx);
                cx.notify();
            });
            visual.run_until_parked();
            assert_eq!(
                bodies(&editor, &mut visual),
                held_bodies,
                "earlier tuning terminal cannot move the source origin"
            );
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let event = terminal(&editor, &mut visual);
        assert_eq!(event.error.is_some(), looping, "{:?}", event.error);
        editor.update(&mut visual, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        if looping {
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project.as_ref(), &before);
                assert_eq!(v.piano_beat_start, -i128::from(beat(1, 1).raw()));
                assert_eq!(v.midi_marker, Some(beat(1, 1).raw()));
                assert!(v.pending_midi_drag.is_none());
            });
            continue;
        }
        assert_eq!(
            bodies(&editor, &mut visual),
            held_bodies,
            "canonical installation must not shift a pixel"
        );
        assert_eq!(
            grid_lines(&mut visual),
            held_grid,
            "source rebasing cannot shift existing snap grid lines"
        );
        let after = editor.read_with(&visual, |v, _| {
            let project = v.state.as_ref().unwrap().project.as_ref();
            let source = project.midi_source(clip).unwrap();
            assert_eq!(source.source_start(), beat(4, 1));
            assert_eq!(source.source_length(), beat(8, 1));
            assert_eq!(project.midi_clip(clip).unwrap().length(), beat(4, 1));
            assert_eq!(project.midi_note(moved).unwrap().start(), beat(7, 2));
            assert_eq!(project.midi_note(moved).unwrap().duration(), beat(1, 1));
            assert_eq!(project.midi_note(held).unwrap().start(), beat(4, 1));
            assert_eq!(
                source
                    .notes()
                    .iter()
                    .find(|n| n.pitch() == 67)
                    .unwrap()
                    .start(),
                if group { beat(9, 2) } else { beat(11, 2) }
            );
            assert_eq!(
                source.notes().len(),
                3,
                "negative crossing author wins against its positive victim"
            );
            assert_eq!(v.piano_beat_start, i128::from(beat(3, 1).raw()));
            assert_eq!(v.midi_marker, Some(beat(5, 1).raw()));
            assert!(v.pending_midi_drag.is_none());
            project.clone()
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.as_ref(), &before)
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.as_ref(), &after)
        });
        let baseline = bodies(&editor, &mut visual);
        let down = position(&editor, &mut visual, 3.75, 60);
        let end = down - point((roll.size.width - px(2.0)) / 8.0, px(0.0));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        repaint(&editor, &mut visual);
        assert_eq!(bodies(&editor, &mut visual), baseline);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.as_ref(), &after);
            assert!(!v.edit_pending());
            assert!(v.pending_midi_drag.is_none());
        });
    }
}

#[gpui::test]
fn negative_keyboard_nudge_uses_one_history_edit_and_exact_origin_ack(cx: &mut TestAppContext) {
    for group in [false, true] {
        let (mut visual, editor, _environment, clip) = fixture(cx);
        let beat = |n, d| BeatTime::from_ratio(n, d).unwrap();
        editor.update(&mut visual, |v, _| {
            assert!(
                v.enqueue(Request::Edit(Command::AddMidiNotes {
                    clip,
                    notes: [(0, 60), (1, 64)]
                        .into_iter()
                        .map(|(start, pitch)| MidiNoteInsert {
                            active: true,
                            start: beat(start, 1),
                            duration: beat(1, 2),
                            pitch,
                            channel: 2,
                            velocity: 71
                        })
                        .collect()
                }))
            )
        });
        pump(&editor, &mut visual);
        let before = editor.update(&mut visual, |v, cx| {
            let project = v.state.as_ref().unwrap().project.as_ref();
            let notes = project.midi_source(clip).unwrap().notes();
            if group {
                v.midi_selection
                    .select_all(notes.iter().map(|n| n.id()))
                    .unwrap();
            } else {
                v.midi_selection.set_single(notes[0].id());
            }
            let project = project.clone();
            v.piano_beat_start = -i128::from(beat(1, 1).raw());
            v.midi_marker = Some(beat(1, 1).raw());
            cx.notify();
            project
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("left");
        editor.read_with(&visual, |v, _| {
            assert!(v.pending_midi_drag.is_some());
            assert_eq!(v.state.as_ref().unwrap().project.as_ref(), &before);
            assert_eq!(v.piano_beat_start, -i128::from(beat(1, 1).raw()));
        });
        let event = terminal(&editor, &mut visual);
        assert!(event.error.is_none(), "{:?}", event.error);
        editor.update(&mut visual, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        let after = editor.read_with(&visual, |v, _| {
            let project = v.state.as_ref().unwrap().project.as_ref();
            let source = project.midi_source(clip).unwrap();
            assert_eq!(source.source_start(), beat(4, 1));
            assert_eq!(source.source_length(), beat(8, 1));
            for old in before.midi_source(clip).unwrap().notes() {
                let new = project.midi_note(old.id()).unwrap();
                assert_eq!(
                    new.start(),
                    if old.pitch() == 60 {
                        beat(15, 4)
                    } else if group {
                        beat(19, 4)
                    } else {
                        beat(5, 1)
                    }
                );
                assert_eq!(
                    (new.duration(), new.velocity(), new.channel()),
                    (old.duration(), old.velocity(), old.channel())
                );
            }
            assert_eq!(v.piano_beat_start, i128::from(beat(3, 1).raw()));
            assert_eq!(v.midi_marker, Some(beat(5, 1).raw()));
            assert!(v.pending_midi_drag.is_none());
            project.clone()
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.as_ref(), &before)
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.as_ref(), &after)
        });
    }
}
