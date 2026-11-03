//! Real ruler input, worker custody and stable destination ranges after repaint.
use super::super::midi_canvas::creation_tests::{fixture, pump};
use super::tests::content_bounds;
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{BeatTime, Command, MidiNoteInsert};
use hex_session::service::Request;
fn drag(visual: &mut VisualTestContext, first: f32, last: f32) {
    let b = content_bounds(visual, "midi-source-ruler");
    let at = |f| point(b.origin.x + b.size.width * f, b.center().y);
    visual.simulate_mouse_down(at(first), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(at(last), Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(at(last), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}
fn assert_visible(
    editor: &Entity<super::EditorView>,
    visual: &mut VisualTestContext,
    first: u64,
    last: u64,
) {
    editor.read_with(visual, |v, _| {
        assert_eq!(v.midi_time_selection_bounds(), Some((first, last)));
        assert!(v.piano_beat_start.raw() <= first);
        assert!(v.piano_beat_start.raw() + v.piano_beat_span.raw() >= last);
        assert!(
            v.midi_selection.is_empty(),
            "time selection never reduces to a note"
        );
    });
    let color: gpui::Background = gpui::Hsla::from(gpui::rgb(crate::theme::time_selection_fill()))
        .opacity(crate::theme::TIME_SELECTION_OPACITY)
        .into();
    let scale = visual.update(|window, _| window.scale_factor());
    for selector in ["midi-source-ruler", "midi-note-roll"] {
        let bounds = content_bounds(visual, selector);
        assert!(
            visual
                .rendered_filled_quads(selector)
                .iter()
                .any(|(quad, c)| c == &color
                    && quad.size.height
                        == gpui::ScaledPixels::from(f32::from(bounds.size.height) * scale)),
            "completed frame retains full-height translucent time highlight in {selector}"
        );
    }
}
#[gpui::test]
fn range_copy_three_times_keeps_destination_after_real_ack_and_later_frames(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, environment, clip) = fixture(cx);
    let beat = BeatTime::ONE.raw();
    editor.update(&mut visual, |v, _| {
        assert!(
            v.enqueue(Request::Edit(Command::AddMidiNotes {
                clip,
                notes: [
                    (beat / 4, beat / 2, 60),
                    (beat, beat / 4, 64),
                    (3 * beat / 2, beat / 4, 67)
                ]
                .into_iter()
                .map(|(start, duration, pitch)| MidiNoteInsert {
                    active: true,
                    start: BeatTime::from_raw(start),
                    duration: BeatTime::from_raw(duration),
                    pitch,
                    channel: 2,
                    velocity: 93
                })
                .collect()
            }))
        )
    });
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    drag(&mut visual, 0.125, 0.5);
    assert_visible(&editor, &mut visual, beat / 2, 2 * beat);
    let mut history = vec![before];
    for n in 1..=3_u64 {
        visual.simulate_keystrokes("cmd-d");
        pump(&editor, &mut visual);
        for _ in 0..3 {
            editor.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
            pump(&editor, &mut visual);
        }
        let first = beat / 2 + n * 3 * beat / 2;
        let last = 2 * beat + n * 3 * beat / 2;
        assert_visible(&editor, &mut visual, first, last);
        history.push(editor.read_with(&visual, |v, _| {
            let s = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
            assert_eq!(s.notes().len(), 3 + 3 * n as usize);
            assert_eq!(s.source_length().raw(), (4 * beat).max(last));
            assert!(s.notes().iter().any(|note| note.start().raw() == first
                && note.pitch() == 60
                && note.duration().raw() == beat / 4));
            v.state.as_ref().unwrap().project.snapshot()
        }));
    }
    for expected in history.iter().rev().skip(1) {
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let mut expected = expected.clone();
            let actual = v.state.as_ref().unwrap().project.snapshot();
            assert!(actual.next_id >= expected.next_id);
            expected.next_id = actual.next_id;
            assert_eq!(actual, expected);
        });
    }
    for expected in history.iter().skip(1) {
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let mut expected = expected.clone();
            let actual = v.state.as_ref().unwrap().project.snapshot();
            expected.next_id = actual.next_id;
            assert_eq!(actual, expected);
        });
    }
    let path = environment.root().join("source-range.hex");
    let saved = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
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
fn silent_ranges_advance_on_real_noop_ack_then_grow_the_entire_destination(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let beat = BeatTime::ONE.raw();
    drag(&mut visual, 0.125, 0.25); // [0.5,1): no notes and no extent growth.
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_keystrokes("cmd-d");
    pump(&editor, &mut visual);
    visual.run_until_parked();
    assert_visible(&editor, &mut visual, beat, 3 * beat / 2);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    drag(&mut visual, 0.75, 1.0);
    visual.simulate_keystrokes("cmd-d");
    pump(&editor, &mut visual);
    visual.run_until_parked();
    assert_visible(&editor, &mut visual, 4 * beat, 5 * beat);
    editor.read_with(&visual, |v, _| {
        let s = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        assert!(s.notes().is_empty());
        assert_eq!(s.source_length().raw(), 5 * beat);
    });
    visual.simulate_keystrokes("cmd-d");
    pump(&editor, &mut visual);
    visual.run_until_parked();
    assert_visible(&editor, &mut visual, 5 * beat, 6 * beat);
}

#[gpui::test]
fn note_selection_takes_duplicate_ownership_and_active_tuning_terminal_rebases_range(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let beat = BeatTime::ONE.raw();
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ONE,
            duration: BeatTime::from_raw(beat / 4),
            pitch: 60,
            channel: 0,
            velocity: 93
        })))
    });
    pump(&editor, &mut visual);
    drag(&mut visual, 0.125, 0.5);
    visual.simulate_keystrokes("cmd-a");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_time_selection_bounds().is_none());
        assert_eq!(v.midi_selection.len(), 1);
    });
    visual.simulate_keystrokes("cmd-d");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_time_selection_bounds().is_none());
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .len(),
            2
        );
    });
    drag(&mut visual, 0.125, 0.5);
    // The edit itself must reserve/finalize the active tuning gesture. Requiring
    // callers to finish first would miss this real enqueue path.
    editor.update(&mut visual, |v, _| {
        let tuning = v.state.as_ref().unwrap().project.tuning();
        v.tuning.client.begin(v.project_generation, tuning).unwrap();
        v.tuning.client.change(tuning.with_enabled(true)).unwrap();
        assert!(v.duplicate_midi_time_selection());
    });
    pump(&editor, &mut visual);
    visual.run_until_parked();
    assert_visible(&editor, &mut visual, 2 * beat, 7 * beat / 2);
    editor.read_with(&visual, |v, _| assert!(v.error.is_none(), "{:?}", v.error));
    let lane = visual.debug_bounds("midi-velocity-lane").unwrap();
    let stem = point(
        lane.origin.x + lane.size.width / 4.0,
        lane.origin.y
            + px(super::super::midi_velocity::handle_y(93, f32::from(lane.size.height)).unwrap()),
    );
    visual.simulate_click(stem, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_time_selection_bounds().is_none());
        assert_eq!(v.midi_selection.len(), 1);
    });
}

#[gpui::test]
fn reversed_ruler_selection_snaps_and_cancel_or_stale_render_keeps_baseline(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    let beat = BeatTime::ONE.raw();
    drag(&mut visual, 0.75, 0.25);
    assert_visible(&editor, &mut visual, beat, 3 * beat);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    for blur in [false, true] {
        let b = content_bounds(&mut visual, "midi-source-ruler");
        let first = point(b.origin.x + b.size.width / 8.0, b.center().y);
        let last = point(b.origin.x + b.size.width / 2.0, b.center().y);
        visual.simulate_mouse_down(first, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(last, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        if blur {
            visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
        } else {
            visual.simulate_keystrokes("escape");
        }
        visual.run_until_parked();
        visual.simulate_mouse_up(last, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        assert_visible(&editor, &mut visual, beat, 3 * beat);
        visual.update(|window, cx| editor.update(cx, |v, _| v.midi_focus.focus(window)));
    }
    drag(&mut visual, 0.11, 0.41);
    assert_visible(&editor, &mut visual, beat / 2, 7 * beat / 4);
    let b = content_bounds(&mut visual, "midi-source-ruler");
    let first = point(b.origin.x + b.size.width * 0.11, b.center().y);
    let last = point(b.origin.x + b.size.width * 0.41, b.center().y);
    let free = Modifiers {
        platform: true,
        ..Default::default()
    };
    visual.simulate_mouse_down(first, MouseButton::Left, free);
    visual.simulate_mouse_move(last, Some(MouseButton::Left), free);
    visual.simulate_mouse_up(last, MouseButton::Left, free);
    visual.run_until_parked();
    let baseline = editor.read_with(&visual, |v, _| {
        let range = v.midi_time_selection_bounds().unwrap();
        assert!((range.0 as f64 / beat as f64 - 0.44).abs() < 0.001);
        assert!((range.1 as f64 / beat as f64 - 1.64).abs() < 0.001);
        range
    });
    // Mutate the view before repaint; old pixels must not capture a new range.
    editor.update(&mut visual, |v, _| v.piano_beat_start = BeatTime::ONE);
    visual.simulate_mouse_down(first, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(last, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(last, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_time_selection_bounds(), Some(baseline));
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn actual_source_ack_cancels_old_pointer_and_blur_before_held_copy_ack_keeps_baseline(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let beat = BeatTime::ONE.raw();
    drag(&mut visual, 0.25, 0.75);
    assert_visible(&editor, &mut visual, beat, 3 * beat);
    let b = content_bounds(&mut visual, "midi-source-ruler");
    let first = point(b.origin.x + b.size.width / 8.0, b.center().y);
    let last = point(b.origin.x + b.size.width / 2.0, b.center().y);
    visual.simulate_mouse_down(first, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(last, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetMidiClipSource {
            clip,
            source_length: BeatTime::from_quarters(4).unwrap(),
            source_start: BeatTime::ONE,
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::from_quarters(4).unwrap(),
            looping: false,
            length: BeatTime::from_quarters(4).unwrap()
        })))
    });
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_mouse_up(last, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    assert_visible(&editor, &mut visual, beat, 3 * beat);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
    });
    visual.simulate_keystrokes("cmd-d");
    let deadline = crate::test_wait::hang_deadline();
    let event = loop {
        if let Ok(event) = editor.read_with(&visual, |v, _| v.service.try_event()) {
            break event;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "actual_source_ack_cancels_old_pointer_and_blur_before_held_copy_ack_keeps_baseline: hang guard expired"
        );
        std::thread::yield_now();
    };
    assert!(event.error.is_none());
    assert!(event.request_complete);
    visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
    visual.run_until_parked();
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    pump(&editor, &mut visual);
    visual.run_until_parked();
    assert_visible(&editor, &mut visual, beat, 3 * beat);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .source_length()
                .raw(),
            5 * beat
        )
    });
}

#[gpui::test]
fn console_range_copy_keeps_arrangement_source_isolated(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, arrangement) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddScene {
            name: "range".into()
        })));
    });
    pump(&editor, &mut visual);
    let scene = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.scenes()[0].id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(
            v.enqueue(Request::Edit(Command::CopyArrangementMidiToSession {
                source: arrangement,
                scene
            }))
        );
    });
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .midi_source(arrangement)
            .unwrap()
            .clone()
    });
    let clip = editor.update(&mut visual, |v, cx| {
        let track = v.selected_track.unwrap();
        let hex_project::SessionCellContent::Midi(clip) = v
            .state
            .as_ref()
            .unwrap()
            .project
            .session_cell(track, scene)
            .unwrap()
        else {
            panic!("midi cell")
        };
        let clip = clip.id();
        v.mode = super::super::EditorMode::Session;
        v.selected_scene = Some(scene);
        v.selected_cell = Some((track, scene));
        v.selected_clip = Some(clip);
        v.piano_view_clip = None;
        cx.notify();
        clip
    });
    visual.run_until_parked();
    let beat = BeatTime::ONE.raw();
    drag(&mut visual, 0.75, 1.0);
    assert_visible(&editor, &mut visual, 3 * beat, 4 * beat);
    visual.simulate_keystrokes("cmd-d");
    pump(&editor, &mut visual);
    visual.run_until_parked();
    assert_visible(&editor, &mut visual, 4 * beat, 5 * beat);
    editor.read_with(&visual, |v, _| {
        let project = &v.state.as_ref().unwrap().project;
        assert_eq!(project.midi_source(arrangement).unwrap(), &before);
        assert_eq!(
            project.midi_source(clip).unwrap().source_length().raw(),
            5 * beat
        );
    });
}
