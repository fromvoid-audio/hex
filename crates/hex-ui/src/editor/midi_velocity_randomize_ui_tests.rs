//! Real lane action, asynchronous admission, exact history and negative controls.
use super::super::midi_canvas::creation_tests::{fixture, pump};
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, point, px};
use hex_project::MidiNoteInsert;
use std::sync::Arc;

fn saved_energy(
    path: &std::path::Path,
    environment: &Arc<super::super::test_environment::Environment>,
) -> f64 {
    struct Custody {
        environment: Arc<super::super::test_environment::Environment>,
        acknowledged: bool,
    }
    impl Drop for Custody {
        fn drop(&mut self) {
            if !self.acknowledged {
                self.environment.retain_for_unacknowledged_setup_failure();
            }
        }
    }
    fn next(service: &hex_session::service::Service) -> hex_session::service::Event {
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
    let service = hex_session::service::Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    let mut custody = Custody {
        environment: Arc::clone(environment),
        acknowledged: false,
    };
    service.submit(Request::Open(path.to_path_buf())).unwrap();
    let mut event = next(&service);
    custody.acknowledged = true;
    let frame = event
        .state
        .project
        .tempo_map()
        .beat_to_frame(BeatTime::from_raw(BeatTime::ONE.raw() / 2))
        .unwrap();
    let mut playback = event.playback.take().unwrap();
    playback.apply(hex_engine::PlaybackCommand::PlayFrom(frame));
    let mut output = [[0.0; 2]; 1024];
    for block in output.chunks_mut(128) {
        playback.render(block).unwrap();
    }
    output
        .into_iter()
        .flatten()
        .map(|sample| f64::from(sample).powi(2))
        .sum()
}

fn originals(view: &EditorView) -> Vec<GroupNote> {
    view.midi_clip()
        .unwrap()
        .notes()
        .iter()
        .map(|note| GroupNote {
            id: note.id(),
            active: note.active(),
            start: note.start().raw(),
            duration: note.duration().raw(),
            pitch: note.pitch(),
            channel: note.channel(),
            velocity: note.velocity(),
        })
        .collect()
}
#[gpui::test]
fn randomizer_selected_only_exact_history_roundtrip_and_playback(cx: &mut TestAppContext) {
    let (mut visual, editor, environment, clip) = fixture(cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::InsertInstrument {
            track: view.selected_track.unwrap(),
            state: hex_project::default_instrument_state().unwrap()
        })));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(
            view.enqueue(Request::Edit(Command::AddMidiNotes {
                clip,
                notes: [1, 64, 127]
                    .into_iter()
                    .enumerate()
                    .map(|(i, velocity)| MidiNoteInsert {
                        active: true,
                        start: BeatTime::from_raw(BeatTime::ONE.raw() * (i as u64 + 1) / 2),
                        duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
                        pitch: 60 + i as u8,
                        channel: 2,
                        velocity,
                    })
                    .collect()
            }))
        );
    });
    pump(&editor, &mut visual);
    let before = editor.update(&mut visual, |view, cx| {
        let notes = originals(view);
        view.midi_selection.set_single(notes[0].id);
        view.midi_selection.toggle(notes[2].id).unwrap();
        view.midi_velocity_randomize.amount = 126;
        cx.notify();
        view.state.as_ref().unwrap().project.snapshot()
    });
    visual.run_until_parked();
    let path = environment.root().join("randomized.hex");
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveNew(path.clone())))
    });
    pump(&editor, &mut visual);
    let before_energy = saved_energy(&path, &environment);
    let original_notes = editor.read_with(&visual, |view, _| originals(view));
    let at = visual
        .debug_bounds("midi-velocity-randomize")
        .unwrap()
        .center();
    visual.simulate_click(at, Modifiers::default());
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |view, _| {
        let notes = originals(view);
        assert_eq!(notes[1].velocity, 64, "unselected note stays unchanged");
        for (index, velocity) in [(0, 72), (2, 95)] {
            assert_eq!(
                notes[index],
                GroupNote {
                    velocity,
                    ..original_notes[index]
                }
            );
        }
        assert_eq!(view.midi_selection.ids(), &[notes[0].id, notes[2].id]);
        view.state.as_ref().unwrap().project.snapshot()
    });
    assert_ne!(before, after);
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before)
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
    });

    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveCurrent))
    });
    pump(&editor, &mut visual);
    let after_energy = saved_energy(&path, &environment);
    assert!(
        before_energy > 0.0 && after_energy > before_energy * 10.0,
        "captured first-note velocity changes real saved/prepared PCM: {before_energy} -> {after_energy}"
    );
    // Opening the actual saved current format recreates prepared playback; it
    // must retain the captured velocities instead of evaluating another seed.
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Open(path)))
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after);
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .is_some()
        );
    });
}

#[gpui::test]
fn randomizer_bounds_determinism_and_stale_or_empty_selection_refuse(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |view, _| {
        assert!(
            view.enqueue(Request::Edit(Command::AddMidiNotes {
                clip,
                notes: [60, 64]
                    .into_iter()
                    .map(|pitch| MidiNoteInsert {
                        active: true,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch,
                        channel: 0,
                        velocity: 1
                    })
                    .collect()
            }))
        )
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::PlaceMidiClip {
            track: view.selected_track.unwrap(),
            start: BeatTime::from_quarters(8).unwrap(),
            length: BeatTime::ONE
        })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, cx| {
        let other_clip = view.selected_clip.unwrap();
        assert_ne!(other_clip, clip);
        view.selected_clip = Some(clip);
        let notes = originals(view);
        let length = view.midi_clip().unwrap().source_length().raw();
        assert!(randomized(&notes, length, 0, 1).unwrap().is_none());
        assert!(randomized(&[], length, 16, 1).is_err());
        assert!(randomized(&notes, length, 127, 1).is_err());
        assert!(randomized(&[notes[0], notes[0]], length, 16, 1).is_err());
        for velocity in [1, 64, 127] {
            for amount in [1, 16, 126] {
                let mut sample = notes[0];
                sample.velocity = velocity;
                for seed in 0..128 {
                    let a = randomized(&[sample], length, amount, seed).unwrap();
                    let b = randomized(&[sample], length, amount, seed).unwrap();
                    assert_eq!(a, b);
                    if let Some(a) = a {
                        assert!((1..=127).contains(&a[0].velocity));
                        assert!(a[0].velocity.abs_diff(velocity) <= amount);
                    }
                }
            }
        }
        for invalid in [
            GroupNote {
                duration: 0,
                ..notes[0]
            },
            GroupNote {
                start: u64::MAX,
                ..notes[0]
            },
            GroupNote {
                velocity: 0,
                ..notes[0]
            },
            GroupNote {
                channel: 16,
                ..notes[0]
            },
        ] {
            assert!(randomized(&[invalid], length, 16, 1).is_err());
        }
        let captured = view.velocity_random_context().unwrap();
        view.midi_selection.clear();
        view.randomize_selected_velocities(captured);
        view.randomize_selected_velocities(view.velocity_random_context().unwrap());
        assert!(!view.edit_pending());
        view.midi_selection.set_single(notes[0].id);
        let captured = view.velocity_random_context().unwrap();
        view.midi_selection.set_single(notes[1].id);
        view.randomize_selected_velocities(captured);
        assert!(!view.edit_pending());
        assert_eq!(view.midi_velocity_randomize.seed, 0);
        for context in [
            Context {
                clip: other_clip,
                ..view.velocity_random_context().unwrap()
            },
            Context {
                generation: view.project_generation + 1,
                ..view.velocity_random_context().unwrap()
            },
            Context {
                revision: view.state.as_ref().unwrap().revision + 1,
                ..view.velocity_random_context().unwrap()
            },
        ] {
            view.randomize_selected_velocities(context);
            assert!(!view.edit_pending());
            assert_eq!(view.midi_velocity_randomize.seed, 0);
        }
        let captured = view.velocity_random_context().unwrap();
        view.midi_velocity_randomize.amount = 0;
        view.randomize_selected_velocities(captured); // stale amount
        assert_eq!(view.midi_velocity_randomize.seed, 0);
        view.randomize_selected_velocities(view.velocity_random_context().unwrap()); // no-op
        assert!(!view.edit_pending());
        assert_eq!(view.midi_velocity_randomize.seed, 1);
        view.midi_velocity_randomize.amount = 126;
        view.midi_velocity_randomize.seed = u64::MAX;
        view.randomize_selected_velocities(view.velocity_random_context().unwrap());
        assert!(!view.edit_pending());
        assert!(view.error.as_ref().unwrap().contains("seed exhausted"));
        cx.notify();
    });
    visual.run_until_parked();
    // At the supported 40px minimum, even ±126 and all controls remain inside
    // their lane. Resize through the real divider, not private height mutation.
    let divider = visual
        .debug_bounds("midi-velocity-resize")
        .unwrap()
        .center();
    let end = divider + point(px(0.0), px(100.0));
    visual.simulate_mouse_down(divider, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let lane = visual.debug_bounds("midi-velocity-lane").unwrap();
    assert_eq!(lane.size.height, px(40.0));
    for selector in [
        "midi-velocity-randomize",
        "midi-velocity-random-less",
        "midi-velocity-random-more",
    ] {
        let button = visual.debug_bounds(selector).unwrap();
        assert!(button.origin.y >= lane.origin.y && button.bottom() <= lane.bottom());
    }
    // Actual bounded controls never enqueue a musical edit.
    for (selector, amount) in [
        ("midi-velocity-random-less", 0),
        ("midi-velocity-random-more", 126),
    ] {
        for _ in 0..18 {
            let at = visual.debug_bounds(selector).unwrap().center();
            visual.simulate_click(at, Modifiers::default());
            visual.run_until_parked();
        }
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.midi_velocity_randomize.amount, amount);
            assert!(!view.edit_pending());
        });
    }
    editor.update(&mut visual, |view, _| {
        view.midi_velocity_randomize.seed = 0;
        let captured = view.velocity_random_context().unwrap();
        assert!(view.enqueue(Request::Edit(Command::RenameTrack {
            track: view.selected_track.unwrap(),
            name: "pending randomizer guard".into()
        })));
        view.randomize_selected_velocities(captured);
        assert_eq!(
            view.midi_velocity_randomize.seed, 0,
            "a pending edit must not consume a random action"
        );
    });
    pump(&editor, &mut visual);
}

#[gpui::test]
fn randomizer_console_selection_uses_its_own_source(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, arrangement) = fixture(cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddMidiNote {
            clip: arrangement,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 3,
            velocity: 1
        })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddScene {
            name: "randomizer scene".into()
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
    let session = editor.update(&mut visual, |view, cx| {
        let track = view.selected_track.unwrap();
        let hex_project::SessionCellContent::Midi(clip) = view
            .state
            .as_ref()
            .unwrap()
            .project
            .session_cell(track, scene)
            .unwrap()
        else {
            panic!("midi cell");
        };
        let clip = clip.id();
        view.mode = super::super::EditorMode::Session;
        view.selected_scene = Some(scene);
        view.selected_cell = Some((track, scene));
        view.selected_clip = Some(clip);
        view.piano_view_clip = None;
        view.midi_selection.set_single(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()[0]
                .id(),
        );
        view.midi_velocity_randomize.amount = 126;
        cx.notify();
        clip
    });
    visual.run_until_parked();
    let at = visual
        .debug_bounds("midi-velocity-randomize")
        .unwrap()
        .center();
    visual.simulate_click(at, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(
            project.midi_source(session).unwrap().notes()[0].velocity(),
            72
        );
        assert_eq!(
            project.midi_source(arrangement).unwrap().notes()[0].velocity(),
            1
        );
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(session)
                .unwrap()
                .notes()[0]
                .velocity(),
            1
        )
    });
}
