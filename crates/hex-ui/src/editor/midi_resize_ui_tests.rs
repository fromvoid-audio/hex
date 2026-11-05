//! Both group edges use real rendered input and atomic canonical history.
use super::creation_tests::{fixture, pump};
use gpui::{CursorStyle, Modifiers, MouseButton, TestAppContext, point, px};
use hex_project::{BeatTime, Command, MidiNoteInsert};
use hex_session::service::Request;

#[gpui::test]
fn group_edges_anchor_each_opposite_edge_and_commit_once_after_release(cx: &mut TestAppContext) {
    for leading in [true, false] {
        let (mut visual, editor, _environment, clip) = fixture(cx);
        editor.update(&mut visual, |view, _| {
            assert!(
                view.enqueue(Request::Edit(Command::AddMidiNotes {
                    clip,
                    notes: [(60, 1, 2), (64, 2, 1)]
                        .into_iter()
                        .map(|(pitch, start, halves)| MidiNoteInsert {
                            active: true,
                            start: BeatTime::from_quarters(start).unwrap(),
                            duration: BeatTime::from_raw(BeatTime::ONE.raw() * halves / 2),
                            pitch,
                            channel: 2,
                            velocity: 93,
                        })
                        .collect()
                }))
            );
        });
        pump(&editor, &mut visual);
        visual.simulate_keystrokes("cmd-a");
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let roll = visual.debug_bounds("midi-note-roll").unwrap();
        // Canvas has a one-pixel border. Pitch60 is row11 of48..72.
        let width = f32::from(roll.size.width) - 2.0;
        let height = f32::from(roll.size.height) - 2.0;
        let down = roll.origin
            + point(
                px(1.0
                    + width * if leading { 0.25 } else { 0.5 }
                    + if leading { 1.0 } else { -1.0 }),
                px(1.0 + height * 11.5 / 24.0),
            );
        let end = down + point(px(width / 8.0 * if leading { -1.0 } else { 1.0 }), px(0.0));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                before,
                "held preview never edits canonical notes"
            )
        });
        if leading {
            visual.simulate_keystrokes("escape");
        } else {
            visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
        }
        visual.run_until_parked();
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.update(|window, cx| editor.update(cx, |v, _| v.midi_focus.focus(window)));
        // Trailing ends snap only while cmd is held; leading edges keep the grid.
        let snap = if leading {
            Modifiers::default()
        } else {
            Modifiers::command()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), snap);
        visual.simulate_mouse_up(end, MouseButton::Left, snap);
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            let notes = v
                .state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes();
            assert_eq!(notes.len(), 2);
            for (note, start, duration) in [
                (&notes[0], if leading { 1 } else { 2 }, 3),
                (&notes[1], if leading { 3 } else { 4 }, 2),
            ] {
                assert_eq!(note.start().raw(), BeatTime::ONE.raw() * start / 2);
                assert_eq!(note.duration().raw(), BeatTime::ONE.raw() * duration / 2);
                assert_eq!((note.channel(), note.velocity()), (2, 93));
            }
            assert_eq!((notes[0].pitch(), notes[1].pitch()), (60, 64));
            assert_eq!(v.midi_selection.len(), 2);
            v.state.as_ref().unwrap().project.snapshot()
        });
        for _ in 0..3 {
            editor.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
        }
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
        });
    }
}

#[gpui::test]
fn explicitly_selected_tiny_note_edges_resize_with_matching_hover_and_stable_id(
    cx: &mut TestAppContext,
) {
    for leading in [true, false] {
        let (mut visual, editor, _environment, clip) = fixture(cx);
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ONE,
                duration: BeatTime::from_raw(BeatTime::ONE.raw() / 64),
                pitch: 60,
                channel: 3,
                velocity: 55,
            })))
        });
        pump(&editor, &mut visual);
        editor.update(&mut visual, |v, cx| {
            v.midi_selection.clear();
            v.piano_beat_span = BeatTime::from_quarters(16).unwrap();
            cx.notify();
        });
        visual.run_until_parked();
        let roll = visual.debug_bounds("midi-note-roll").unwrap();
        let (id, left, right, y, before) = editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            let source = state.project.midi_source(clip).unwrap();
            let note = &source.notes()[0];
            let scene = super::super::midi_scene::MidiScene::build(
                super::super::midi_scene::SceneKey {
                    generation: v.project_generation,
                    revision: state.revision,
                    clip,
                    beat_start: v.piano_beat_start,
                    beat_span: v.piano_beat_span.raw(),
                    pitch_low: v.piano_pitch_low,
                    pitch_rows: v.piano_pitch_rows,
                },
                source.notes(),
            );
            let geometry = scene.note(note.id()).unwrap();
            let (left, right) =
                super::super::midi_scene::pixel_bounds(geometry, f32::from(roll.size.width) - 2.0)
                    .unwrap();
            assert!(
                (right - left - 4.0).abs() < 0.01,
                "exercise the minimum hit halo"
            );
            (
                note.id(),
                left,
                right,
                (geometry.y + 0.5 / f32::from(v.piano_pitch_rows))
                    * (f32::from(roll.size.height) - 2.0),
                state.project.snapshot(),
            )
        });
        let center = roll.origin + point(px(1.0 + (left + right) / 2.0), px(1.0 + y));
        visual.simulate_click(center, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.midi_selection.primary(), Some(id));
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        let down = roll.origin
            + point(
                px(1.0 + if leading { left + 0.25 } else { right - 0.25 }),
                px(1.0 + y),
            );
        visual.simulate_mouse_move(down, None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(if leading {
                CursorStyle::ResizeLeft
            } else {
                CursorStyle::ResizeRight
            })
        );
        let end = down
            + point(
                (roll.size.width - px(2.0)) / 64.0 * if leading { -1.0 } else { 1.0 },
                px(0.0),
            );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default()); // release position, no motion event
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let note = v.state.as_ref().unwrap().project.midi_note(id).unwrap();
            assert_eq!(
                note.start().raw(),
                if leading {
                    BeatTime::ONE.raw() * 3 / 4
                } else {
                    BeatTime::ONE.raw()
                }
            );
            assert_eq!(
                note.duration().raw(),
                BeatTime::ONE.raw() / 64 + BeatTime::ONE.raw() / 4
            );
            assert_eq!((note.pitch(), note.channel(), note.velocity()), (60, 3, 55));
            assert_eq!(v.midi_selection.primary(), Some(id));
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
}

#[gpui::test]
fn group_leading_resize_paints_and_commits_the_same_destructive_prefix(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(
            v.enqueue(Request::Edit(Command::AddMidiNotes {
                clip,
                notes: [(0, 60), (1, 60), (2, 64)]
                    .into_iter()
                    .map(|(start, pitch)| MidiNoteInsert {
                        active: true,
                        start: BeatTime::from_quarters(start).unwrap(),
                        duration: BeatTime::ONE,
                        pitch,
                        channel: 0,
                        velocity: 90,
                    })
                    .collect()
            }))
        )
    });
    pump(&editor, &mut visual);
    let (victim, before) = editor.update(&mut visual, |v, cx| {
        let notes = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes();
        let victim = notes[0].id();
        v.midi_selection.set_single(notes[1].id());
        v.midi_selection.toggle(notes[2].id()).unwrap();
        cx.notify();
        (victim, v.state.as_ref().unwrap().project.snapshot())
    });
    visual.run_until_parked();
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let width = f32::from(roll.size.width) - 2.0;
    let height = f32::from(roll.size.height) - 2.0;
    let down = roll.origin + point(px(2.0 + width / 4.0), px(1.0 + height * 11.5 / 24.0));
    let end = down - point(px(width / 8.0), px(0.0));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
    let secondary: gpui::Background =
        gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Secondary)).into();
    let retained: Vec<_> = visual
        .rendered_filled_quads("midi-note-roll")
        .into_iter()
        .filter(|(_, color)| color == &secondary)
        .collect();
    assert_eq!(
        retained.len(),
        1,
        "the old same-pitch note retains only its left prefix"
    );
    assert!((retained[0].0.size.width / scale - width / 8.0).abs() < 0.05);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        assert_eq!(source.notes().len(), 3);
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_note(victim)
                .unwrap()
                .duration()
                .raw(),
            BeatTime::ONE.raw() / 2
        );
        assert_eq!(source.source_length(), BeatTime::from_quarters(4).unwrap());
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}
