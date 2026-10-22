//! Rendered velocity gestures through canonical worker/history and saved playback.
use super::super::{
    midi_canvas::creation_tests::{fixture, pump},
    test_environment::Environment,
};
use super::*;
use gpui::{Entity, Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, ClipId, MidiNoteInsert};
use hex_session::service::{Event, Service};
use std::{
    path::Path,
    time::{Duration, Instant},
};

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
        view.piano_beat_start = BeatTime::ZERO;
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
    let (before_saved, before_energy) = saved_energy(&path, &mut custody);
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
    let (reopened, edited_energy) = saved_energy(&path, &mut custody);
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
            view.piano_beat_start.raw()
        );
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
    });
}
fn next(service: &Service) -> Event {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
}
fn saved_energy(path: &Path, custody: &mut FileCustody) -> (hex_project::Snapshot, f64) {
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
    playback.apply(hex_engine::PlaybackCommand::PlayFrom(frame));
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
