//! Rendered velocity gestures through canonical worker/history and saved playback.
use super::super::{
    midi_canvas::creation_tests::{fixture, pump},
    test_environment::Environment,
};
use super::*;
use gpui::{
    Entity, Modifiers, MouseUpEvent, ScrollDelta, ScrollWheelEvent, TestAppContext,
    VisualTestContext,
};
use hex_project::{BeatTime, ClipId, Command, MidiNoteInsert};
use hex_session::service::{Event, Request, Service};
use std::{path::Path, time::Instant};

fn setup(
    cx: &mut TestAppContext,
) -> (
    VisualTestContext,
    Entity<EditorView>,
    Arc<Environment>,
    ClipId,
) {
    let (mut visual, editor, environment, clip) = fixture(cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::InsertInstrument {
            track: view.selected_track.unwrap(),
            state: hex_project::default_instrument_state().unwrap()
        })))
    });
    pump(&editor, &mut visual);
    let notes = [
        (60, BeatTime::ONE.raw() / 2, 37),
        (64, BeatTime::ONE.raw(), 93),
        (0, BeatTime::ONE.raw(), 93),
    ]
    .into_iter()
    .map(|(pitch, start, velocity)| MidiNoteInsert {
        active: true,
        start: BeatTime::from_raw(start),
        duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
        pitch,
        channel: 2,
        velocity,
    })
    .collect();
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddMidiNotes { clip, notes })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, cx| {
        view.midi_selection.clear();
        view.piano_beat_start = 0;
        view.piano_beat_span = BeatTime::from_quarters(4).unwrap();
        cx.notify();
    });
    visual.run_until_parked();
    (visual, editor, environment, clip)
}
fn handle(visual: &mut VisualTestContext, quarter: f32, velocity: u8) -> gpui::Point<gpui::Pixels> {
    let bounds = visual.debug_bounds("midi-velocity-lane").unwrap();
    point(
        bounds.origin.x + bounds.size.width * (quarter / 4.0),
        bounds.origin.y
            + px(midi_velocity::handle_y(velocity, f32::from(bounds.size.height)).unwrap()),
    )
}
#[gpui::test]
fn velocity_pointer_preview_release_history_and_current_format_playback(cx: &mut TestAppContext) {
    let (mut visual, editor, environment, clip) = setup(cx);
    let path = environment.root().join("velocity.hex");
    let mut custody = FileCustody {
        environment: Arc::clone(&environment),
        acknowledged: false,
    };
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveNew(path.clone())))
    });
    pump(&editor, &mut visual);
    custody.acknowledged = true;
    let (before_saved, before_energy) = saved_energy(&path, &mut custody, None);
    let start = handle(&mut visual, 0.5, 37);
    visual.simulate_mouse_move(start, None, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        start - point(px(0.0), px(14.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let note = &view
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes()[0];
        assert_eq!(
            note.velocity(),
            37,
            "preview must not mutate canonical state"
        );
        assert_eq!(
            view.midi_velocity
                .gesture
                .as_ref()
                .unwrap()
                .preview(note.id()),
            Some(69)
        );
    });
    // A release without a final motion still uses its actual position.
    visual.simulate_mouse_up(
        start - point(px(0.0), px(16.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    pump(&editor, &mut visual);
    let edited = editor.read_with(&visual, |view, _| {
        let source = view
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap();
        assert_eq!(
            source
                .notes()
                .iter()
                .map(|note| note.velocity())
                .collect::<Vec<_>>(),
            [73, 93, 93]
        );
        assert_eq!(view.midi_selection.ids(), [source.notes()[0].id()]);
        view.state.as_ref().unwrap().project.snapshot()
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.snapshot(),
            before_saved
        )
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), edited)
    });
    custody.acknowledged = false;
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveCurrent))
    });
    pump(&editor, &mut visual);
    custody.acknowledged = true;
    let (reopened, edited_energy) = saved_energy(&path, &mut custody, None);
    assert_eq!(reopened, edited);
    assert!(
        before_energy > 0.0 && edited_energy > before_energy * 1.2,
        "the saved velocity edit must affect actual prepared playback: {before_energy} -> {edited_energy}"
    );
}
#[gpui::test]
fn velocity_hidden_pitch_handles_cycle_and_group_drag_preserves_offsets(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = setup(cx);
    let at = handle(&mut visual, 1.0, 93);
    let mut chosen = Vec::new();
    for _ in 0..2 {
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        chosen.push(editor.read_with(&visual, |view, _| view.midi_selection.primary().unwrap()));
    }
    assert_ne!(chosen[0], chosen[1]);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_note(chosen[1])
                .unwrap()
                .pitch(),
            0,
            "the hidden pitch must remain individually editable"
        )
    });
    visual.simulate_keystrokes("cmd-a");
    visual.run_until_parked();
    let start = handle(&mut visual, 0.5, 37);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(
        start - point(px(0.0), px(7.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let source = view
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap();
        assert_eq!(
            source
                .notes()
                .iter()
                .map(|note| note.velocity())
                .collect::<Vec<_>>(),
            [53, 109, 109]
        );
        assert_eq!(view.midi_selection.len(), 3);
    });
}
#[gpui::test]
fn velocity_escape_blur_and_viewport_changes_cancel_without_note_edits(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = setup(cx);
    let before = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    for action in 0..3 {
        let start = handle(&mut visual, 0.5, 37);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            start - point(px(0.0), px(14.0)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        visual.run_until_parked();
        match action {
            0 => visual.simulate_keystrokes("escape"),
            1 => {
                visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
            }
            _ => editor.update(&mut visual, |view, cx| {
                view.piano_beat_span = BeatTime::from_quarters(8).unwrap();
                cx.notify();
            }),
        }
        visual.run_until_parked();
        visual.simulate_mouse_up(
            start - point(px(0.0), px(14.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.midi_velocity.gesture.is_none());
            assert!(!view.edit_pending());
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
}
#[gpui::test]
fn velocity_resize_collapse_and_wheel_keep_note_space_and_time_alignment(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = setup(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let lane = visual.debug_bounds("midi-velocity-lane").unwrap();
    assert_eq!(roll.origin.x, lane.origin.x);
    assert_eq!(roll.size.width, lane.size.width);
    let before = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    let divider = visual
        .debug_bounds("midi-velocity-resize")
        .unwrap()
        .center();
    visual.simulate_mouse_down(divider, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        divider - point(px(0.0), px(16.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.simulate_mouse_up(
        divider - point(px(0.0), px(16.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert_eq!(
        visual
            .debug_bounds("midi-velocity-lane")
            .unwrap()
            .size
            .height,
        px(80.0)
    );
    assert_eq!(
        visual.debug_bounds("midi-note-roll").unwrap().size.height,
        roll.size.height
    );
    let toggle = visual
        .debug_bounds("midi-velocity-toggle")
        .unwrap()
        .center();
    visual.simulate_click(toggle, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("midi-velocity-lane").is_none());
    assert_eq!(
        visual.debug_bounds("midi-note-roll").unwrap().size.height,
        roll.size.height
    );
    let toggle = visual
        .debug_bounds("midi-velocity-toggle")
        .unwrap()
        .center();
    visual.simulate_click(toggle, Modifiers::default());
    visual.run_until_parked();
    let lane = visual.debug_bounds("midi-velocity-lane").unwrap();
    visual.simulate_event(ScrollWheelEvent {
        position: lane.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.piano_beat_span.raw() < BeatTime::ONE.raw() * 4);
        assert_eq!(
            view.midi_velocity.scene.as_ref().unwrap().key.span,
            view.piano_beat_span.raw()
        );
        assert_eq!(
            view.midi_velocity.scene.as_ref().unwrap().key.start,
            view.piano_beat_start
        );
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
    });
}
fn next(service: &Service) -> Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(
            std::time::Instant::now() < until,
            "next: hang guard expired"
        );
        std::thread::yield_now();
    }
}
fn saved_energy(
    path: &Path,
    custody: &mut FileCustody,
    session_clip: Option<ClipId>,
) -> (hex_project::Snapshot, f64) {
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    custody.acknowledged = false;
    service.submit(Request::Open(path.to_path_buf())).unwrap();
    let mut event = next(&service);
    custody.acknowledged = true;
    let frame = event
        .state
        .project
        .tempo_map()
        .beat_to_frame(BeatTime::from_raw(BeatTime::ONE.raw() / 2))
        .unwrap();
    let mut playback = event
        .playback
        .take()
        .expect("accepted worker-prepared playback");
    if let Some(clip) = session_clip {
        playback
            .try_apply_action(hex_engine::PlaybackAction::LaunchAndPlay(
                hex_engine::SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: hex_engine::LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        let mut remaining = frame;
        let mut preroll = [[0.0; 2]; 128];
        while remaining > 0 {
            let count = remaining.min(preroll.len() as u64) as usize;
            playback.render(&mut preroll[..count]).unwrap();
            remaining -= count as u64;
        }
    } else {
        playback.apply(hex_engine::PlaybackCommand::PlayFrom(frame));
    }
    let mut output = [[0.0; 2]; 1024];
    for chunk in output.chunks_mut(128) {
        playback.render(chunk).unwrap();
    }
    (
        event.state.project.snapshot(),
        output
            .into_iter()
            .flatten()
            .map(|sample| f64::from(sample).powi(2))
            .sum(),
    )
}
struct FileCustody {
    environment: Arc<Environment>,
    acknowledged: bool,
}
impl Drop for FileCustody {
    fn drop(&mut self) {
        if !self.acknowledged {
            self.environment.retain_for_unacknowledged_setup_failure();
        }
    }
}

#[gpui::test]
fn velocity_session_clip_edit_round_trips_and_changes_launched_audio(cx: &mut TestAppContext) {
    let (mut visual, editor, environment, arrangement) = setup(cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddScene {
            name: "velocity scene".into()
        })))
    });
    pump(&editor, &mut visual);
    let scene = editor.read_with(&visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .scenes()
            .last()
            .unwrap()
            .id()
    });
    editor.update(&mut visual, |view, _| {
        assert!(
            view.enqueue(Request::Edit(Command::CopyArrangementMidiToSession {
                source: arrangement,
                scene
            }))
        )
    });
    pump(&editor, &mut visual);
    let clip = editor.update(&mut visual, |view, cx| {
        let track = view.selected_track.unwrap();
        let hex_project::SessionCellContent::Midi(cell) = view
            .state
            .as_ref()
            .unwrap()
            .project
            .session_cell(track, scene)
            .unwrap()
        else {
            panic!("midi cell")
        };
        let clip = cell.id();
        view.mode = super::super::EditorMode::Session;
        view.selected_scene = Some(scene);
        view.selected_cell = Some((track, scene));
        view.selected_clip = Some(clip);
        view.piano_view_clip = None;
        view.midi_selection.clear();
        cx.notify();
        clip
    });
    visual.run_until_parked();
    let path = environment.root().join("session-velocity.hex");
    let mut custody = FileCustody {
        environment: Arc::clone(&environment),
        acknowledged: false,
    };
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveNew(path.clone())))
    });
    pump(&editor, &mut visual);
    custody.acknowledged = true;
    let (_, before_energy) = saved_energy(&path, &mut custody, Some(clip));
    let start = handle(&mut visual, 0.5, 37);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(
        start - point(px(0.0), px(16.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    pump(&editor, &mut visual);
    let edited = editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()[0]
                .velocity(),
            73
        );
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(arrangement)
                .unwrap()
                .notes()[0]
                .velocity(),
            37
        );
        view.state.as_ref().unwrap().project.snapshot()
    });
    custody.acknowledged = false;
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveCurrent))
    });
    pump(&editor, &mut visual);
    custody.acknowledged = true;
    let (reopened, after_energy) = saved_energy(&path, &mut custody, Some(clip));
    assert_eq!(reopened, edited);
    assert!(
        before_energy > 0.0 && after_energy > before_energy * 1.2,
        "launched saved Session audio: {before_energy} -> {after_energy}"
    );
}

#[gpui::test]
fn velocity_focus_blur_repaints_stationary_preview_without_another_input(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = setup(cx);
    let start = handle(&mut visual, 0.5, 37);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let top = |visual: &mut VisualTestContext| {
        let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
        let primary: gpui::Background = rgb(theme::color(theme::PaletteRole::Primary)).into();
        visual
            .rendered_filled_quads("midi-velocity-lane")
            .into_iter()
            .find(|(bounds, color)| {
                color == &primary
                    && (bounds.size.height / scale - 4.0).abs() < 0.01
                    && (bounds.size.width / scale - 6.0).abs() < 0.01
                    && (bounds.origin.x / scale - f32::from(start.x) + 3.0).abs() < 0.05
            })
            .map(|(bounds, _)| bounds.origin.y / scale)
            .expect("completed selected velocity handle")
    };
    let canonical = top(&mut visual);
    visual.simulate_mouse_move(
        start - point(px(0.0), px(14.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(top(&mut visual) < canonical - 10.0);
    // Focus changes after paint. No motion, mouse-up, manual notify or worker pump
    // may disguise a missing cancellation repaint.
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.midi_velocity.gesture.is_none())
    });
    assert!(
        (top(&mut visual) - canonical).abs() < 0.05,
        "a cancelled preview must disappear from the completed scene without another input"
    );
}

fn lane_at(
    visual: &mut VisualTestContext,
    beat: f32,
    start: f32,
    span: f32,
    y: f32,
) -> gpui::Point<gpui::Pixels> {
    let bounds = visual.debug_bounds("midi-velocity-lane").unwrap();
    point(
        bounds.left() + bounds.size.width * ((beat - start) / span),
        bounds.top() + px(y),
    )
}
fn painted_time_in_lane(visual: &mut VisualTestContext) {
    let color: gpui::Background = gpui::Hsla::from(rgb(theme::time_selection_fill()))
        .opacity(theme::TIME_SELECTION_OPACITY)
        .into();
    assert!(
        visual
            .rendered_filled_quads("midi-velocity-lane")
            .iter()
            .any(|(_, c)| c == &color)
    );
}
#[gpui::test]
fn velocity_empty_drag_shares_time_range_and_duplicate_destination_after_ack(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = setup(cx);
    for (start, span, reverse) in [(0.0, 4.0, false), (0.25, 2.0, true)] {
        editor.update(&mut visual, |v, cx| {
            v.piano_beat_start = i128::from((start * BeatTime::ONE.raw() as f32) as u64);
            v.piano_beat_span = BeatTime::from_raw((span * BeatTime::ONE.raw() as f32) as u64);
            cx.notify();
        });
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let first = lane_at(
            &mut visual,
            if reverse { 1.5 } else { 0.25 },
            start,
            span,
            2.0,
        );
        let last = lane_at(
            &mut visual,
            if reverse { 0.25 } else { 1.5 },
            start,
            span,
            2.0,
        );
        visual.simulate_mouse_down(first, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(last, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        painted_time_in_lane(&mut visual);
        visual.simulate_mouse_up(last, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.midi_time_selection_bounds(),
                Some((BeatTime::ONE.raw() / 4, 3 * BeatTime::ONE.raw() / 2))
            );
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert!(!v.edit_pending());
        });
        visual.simulate_keystrokes("cmd-d");
        pump(&editor, &mut visual);
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        painted_time_in_lane(&mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.midi_time_selection_bounds(),
                Some((3 * BeatTime::ONE.raw() / 2, 11 * BeatTime::ONE.raw() / 4))
            );
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .midi_source(clip)
                    .unwrap()
                    .notes()
                    .len(),
                6
            );
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let actual = v.state.as_ref().unwrap().project.snapshot();
            let mut expected = before.clone();
            assert!(actual.next_id >= expected.next_id);
            expected.next_id = actual.next_id;
            assert_eq!(actual, expected)
        });
    }
}
#[gpui::test]
fn velocity_shift_marquee_highlights_both_views_and_selects_hidden_pitch_for_group_edit(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = setup(cx);
    let (all_ids, before) = editor.update(&mut visual, |v, cx| {
        let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        let ids = source.notes().iter().map(|n| n.id()).collect::<Vec<_>>();
        v.midi_selection.set_single(ids[0]);
        cx.notify();
        (ids, v.state.as_ref().unwrap().project.snapshot())
    });
    visual.run_until_parked();
    let first = lane_at(&mut visual, 0.85, 0.0, 4.0, 2.0);
    let last = lane_at(&mut visual, 1.15, 0.0, 4.0, 54.0);
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(first, MouseButton::Left, shift);
    visual.simulate_mouse_move(last, Some(MouseButton::Left), shift);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.ids(), all_ids);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    let primary: gpui::Background = rgb(theme::color(theme::PaletteRole::Primary)).into();
    let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
    let handles = visual
        .rendered_filled_quads("midi-velocity-lane")
        .into_iter()
        .filter(|(b, c)| c == &primary && (b.size.height / scale - 4.0).abs() < 0.01)
        .count();
    assert_eq!(
        handles, 3,
        "baseline and both intersected stems must preview together"
    );
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let rows = editor.read_with(&visual, |v, _| v.piano_pitch_rows);
    let height = ((f32::from(roll.size.height) - 2.0) / f32::from(rows) - 2.0).max(1.0);
    assert!(
        visual
            .rendered_filled_quads("midi-note-roll")
            .iter()
            .filter(|(b, c)| c == &primary && (b.size.height / scale - height).abs() < 0.1)
            .count()
            >= 2,
        "both visible candidate note bodies must paint selected before release"
    );
    visual.simulate_mouse_up(last, MouseButton::Left, shift);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.ids(), all_ids);
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .iter()
                .any(|n| n.pitch() == 0 && v.midi_selection.contains(n.id()))
        );
    });
    // A stem, below its handle, retains direct velocity editing priority.
    let stem = lane_at(&mut visual, 0.5, 0.0, 4.0, 54.0);
    visual.simulate_mouse_down(stem, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(
        stem - point(px(0.0), px(4.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        assert_eq!(
            source
                .notes()
                .iter()
                .map(|n| n.velocity())
                .collect::<Vec<_>>(),
            [46, 102, 102]
        );
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.simulate_keystrokes("cmd-d");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .len(),
            6
        );
        assert_eq!(v.midi_selection.len(), 3);
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    visual.simulate_keystrokes("cmd-a");
    visual.run_until_parked();
    let random = visual.debug_bounds("midi-velocity-randomize").unwrap();
    visual.simulate_click(random.center(), Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_ne!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .iter()
                .map(|n| n.velocity())
                .collect::<Vec<_>>(),
            [37, 93, 93]
        )
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let actual = v.state.as_ref().unwrap().project.snapshot();
        let mut expected = before.clone();
        assert!(actual.next_id >= expected.next_id);
        expected.next_id = actual.next_id;
        assert_eq!(actual, expected);
    });
}
#[gpui::test]
fn velocity_range_and_marquee_cancel_on_escape_blur_resize_and_stale_view(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = setup(cx);
    for shift in [false, true] {
        for cancel in ["escape", "blur", "resize", "viewport", "revision", "clip"] {
            visual.update(|window, cx| {
                editor.update(cx, |v, cx| {
                    v.midi_focus.focus(window);
                    v.clear_midi_time_selection();
                    cx.notify();
                })
            });
            visual.run_until_parked();
            let modifiers = Modifiers {
                shift,
                ..Modifiers::default()
            };
            let first = lane_at(&mut visual, 0.85, 0.0, 4.0, 2.0);
            let last = lane_at(&mut visual, 1.15, 0.0, 4.0, 54.0);
            visual.simulate_mouse_down(first, MouseButton::Left, modifiers);
            visual.simulate_mouse_move(last, Some(MouseButton::Left), modifiers);
            visual.run_until_parked();
            let mut before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            match cancel {
                "escape" => visual.simulate_keystrokes("escape"),
                "blur" => visual.update(|window, cx| {
                    editor.update(cx, |v, cx| {
                        v.focus.focus(window);
                        cx.notify();
                    })
                }),
                "resize" => visual.simulate_resize(gpui::size(px(1100.0), px(700.0))),
                "revision" | "clip" => {
                    editor.update(&mut visual, |v, _| {
                        let track = v.selected_track.unwrap();
                        let command = if cancel == "revision" {
                            Command::RenameTrack {
                                track,
                                name: format!("new revision {shift}"),
                            }
                        } else {
                            Command::PlaceMidiClip {
                                track,
                                start: BeatTime::from_quarters(8 + 2 * u64::from(shift)).unwrap(),
                                length: BeatTime::ONE,
                            }
                        };
                        assert!(v.enqueue(Request::Edit(command)));
                    });
                    pump(&editor, &mut visual);
                    before = editor
                        .read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
                }
                _ => editor.update(&mut visual, |v, cx| {
                    v.piano_beat_start = i128::from(BeatTime::ONE.raw());
                    cx.notify();
                }),
            }
            visual.run_until_parked();
            visual.simulate_mouse_up(last, MouseButton::Left, modifiers);
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert!(v.midi_velocity.marquee.is_none());
                assert!(!v.midi_velocity_time_selection_active());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
                assert!(
                    v.midi_selection.is_empty(),
                    "shift={shift} cancel={cancel} ids={:?}",
                    v.midi_selection.ids()
                );
                assert!(
                    v.midi_time_selection_bounds().is_none(),
                    "shift={shift} cancel={cancel}"
                );
            });
            editor.update(&mut visual, |v, cx| {
                v.selected_clip = Some(clip);
                v.piano_beat_start = 0;
                cx.notify();
            });
            visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
            visual.run_until_parked();
        }
    }
}

#[gpui::test]
fn velocity_stationary_click_outside_release_and_collapse_keep_capture_bounded(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, _) = setup(cx);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let first = lane_at(&mut visual, 1.75, 0.0, 4.0, 2.0);
    visual.simulate_click(first, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_time_selection_bounds().is_none());
        assert_eq!(v.midi_marker, Some(7 * BeatTime::ONE.raw() / 4));
    });
    let bounds = visual.debug_bounds("midi-velocity-lane").unwrap();
    let outside = bounds.bottom_right() + point(px(40.0), px(20.0));
    visual.simulate_mouse_down(first, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.midi_time_selection_bounds(),
            Some((7 * BeatTime::ONE.raw() / 4, 4 * BeatTime::ONE.raw()))
        )
    });
    for shift in [false, true] {
        let modifiers = Modifiers {
            shift,
            ..Modifiers::default()
        };
        let first = lane_at(&mut visual, 0.85, 0.0, 4.0, 2.0);
        let last = lane_at(&mut visual, 1.15, 0.0, 4.0, 54.0);
        visual.simulate_mouse_down(first, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(last, Some(MouseButton::Left), modifiers);
        visual.run_until_parked();
        let toggle = visual.debug_bounds("midi-velocity-toggle").unwrap();
        visual.simulate_mouse_down(toggle.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        // Both captured selection owners block a repeated press on collapse.
        editor.read_with(&visual, |v, _| {
            assert!(v.midi_velocity.open);
            if shift {
                assert!(v.midi_velocity.marquee.is_some());
            } else {
                assert!(v.midi_velocity_time_selection_active());
            }
        });
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(last, MouseButton::Left, modifiers);
        visual.simulate_click(toggle.center(), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(!v.midi_velocity.open);
            assert!(v.midi_velocity.marquee.is_none());
            assert!(!v.midi_velocity_time_selection_active());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        let toggle = visual.debug_bounds("midi-velocity-toggle").unwrap();
        visual.simulate_click(toggle.center(), Modifiers::default());
        visual.run_until_parked();
    }
}

#[gpui::test]
fn externally_collapsed_velocity_lane_retires_held_time_selection(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _) = setup(cx);
    let first = lane_at(&mut visual, 0.25, 0., 4., 2.);
    let last = lane_at(&mut visual, 1.5, 0., 4., 2.);
    visual.simulate_mouse_down(first, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(last, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(last, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let prior = editor.read_with(&visual, |v, _| {
        (
            v.midi_time_selection_bounds(),
            v.midi_marker,
            v.state.as_ref().unwrap().project.snapshot(),
        )
    });
    visual.simulate_mouse_down(last, MouseButton::Left, Modifiers::default());
    assert!(visual.update(|w, _| w.is_pointer_dragging()));
    // External visibility changes do not dispatch through the pointer lease.
    editor.update(&mut visual, |v, cx| {
        v.midi_velocity.open = false;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(!visual.update(|w, _| w.is_pointer_dragging()));
    visual.simulate_mouse_up(first, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(!v.midi_velocity_time_selection_active());
        assert_eq!(
            (
                v.midi_time_selection_bounds(),
                v.midi_marker,
                v.state.as_ref().unwrap().project.snapshot()
            ),
            prior
        );
    });
}

#[path = "midi_velocity_preview_ui_tests.rs"]
mod preview_tests;
