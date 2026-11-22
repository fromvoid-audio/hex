//! Public batch entry-point regressions; no private Project fields or validator.
use hex_project::{Command, Created, Editor, Invalid, Project, ReturnId, RoutingOwner, TrackId};

fn track(editor: &mut Editor) -> TrackId {
    let Created::Track(id) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "track".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    id
}
fn bus(editor: &mut Editor) -> ReturnId {
    let Created::ReturnTrack(id) = editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    id
}
fn audio_fixture() -> (Project, TrackId, ReturnId) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let t = track(&mut e);
    let r1 = bus(&mut e);
    let r2 = bus(&mut e);
    e.execute(Command::SetReturnFeed {
        source: r1,
        destination: r2,
        gain: 1.0,
    })
    .unwrap();
    e.execute(Command::SetAudioFeed {
        source: RoutingOwner::Return(r2),
        destination: t,
        gain: 1.0,
    })
    .unwrap();
    (e.project().clone(), t, r1)
}
fn midi_fixture() -> (Project, TrackId) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let source = track(&mut e);
    let destination = track(&mut e);
    e.execute(Command::SetMidiSource {
        destination,
        source: Some(source),
    })
    .unwrap();
    (e.project().clone(), destination)
}
fn cycle(track: TrackId, return_track: ReturnId, gain: f32) -> Command {
    Command::SetReturnSend {
        track,
        return_track,
        gain,
    }
}

fn refuses_without_preparation_or_history_change(
    project: Project,
    invalid: Command,
    message: &'static str,
) {
    assert_eq!(Project::from_snapshot(project.snapshot()).unwrap(), project);
    let mut e = Editor::new(project.clone(), 4).unwrap();
    e.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let before = e.project().clone();
    e.execute(Command::MasterGain { gain: 0.25 }).unwrap();
    let redo = e.project().clone();
    assert!(e.undo());
    assert!(e.can_undo() && e.can_redo());
    let mut calls = 0;
    let result = e.execute_many_prepared(
        vec![
            Command::AddTrack {
                name: "speculative id".into(),
            },
            invalid,
        ],
        |_, _| {
            calls += 1;
            Ok::<_, Invalid>(())
        },
    );
    assert_eq!(result, Err(Invalid(message)));
    assert_eq!(
        calls, 0,
        "model refusal must precede preparation/publication"
    );
    assert_eq!(
        e.project(),
        &before,
        "including the allocator high-water mark"
    );
    assert!(e.can_undo() && e.can_redo());
    assert!(e.redo());
    assert_eq!(e.project(), &redo);
    assert!(!e.can_redo());
    assert!(e.undo());
    assert_eq!(e.project(), &before);
    assert!(e.undo());
    assert_eq!(e.project(), &project);
    assert!(!e.can_undo());
}
#[test]
fn batch_rejects_return_feed_track_input_send_cycle_before_preparation() {
    let (project, t, r1) = audio_fixture();
    refuses_without_preparation_or_history_change(
        project,
        cycle(t, r1, 1.0),
        "routing cycles are unsupported",
    );
}
#[test]
fn batch_rejects_midi_destination_without_instrument_before_preparation() {
    let (project, destination) = midi_fixture();
    refuses_without_preparation_or_history_change(
        project,
        Command::RemoveInstrument { track: destination },
        "midi destination requires an instrument",
    );
}
#[test]
fn valid_final_audio_batch_can_pass_through_a_transient_cycle() {
    let (project, t, r1) = audio_fixture();
    let mut e = Editor::new(project.clone(), 4).unwrap();
    assert!(
        e.execute(cycle(t, r1, 1.0)).is_err(),
        "single transition remains invalid"
    );
    let mut calls = 0;
    let (results, ()) = e
        .execute_many_prepared(
            vec![
                cycle(t, r1, 1.0),
                cycle(t, r1, 0.0),
                Command::MasterGain { gain: 0.5 },
            ],
            |candidate, results| {
                calls += 1;
                assert_eq!(results, &[Created::None; 3]);
                assert_eq!(
                    Project::from_snapshot(candidate.snapshot()).unwrap(),
                    *candidate
                );
                Ok::<_, Invalid>(())
            },
        )
        .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(results.len(), 3);
    let after = e.project().clone();
    assert_eq!(after.master_gain(), 0.5);
    assert!(e.undo());
    assert_eq!(e.project(), &project);
    assert!(!e.can_undo());
    assert!(e.redo());
    assert_eq!(e.project(), &after);
    // A final-state no-op still prepares once and must preserve the redo branch.
    assert!(e.undo());
    calls = 0;
    e.execute_many_prepared(
        vec![cycle(t, r1, 1.0), cycle(t, r1, 0.0)],
        |candidate, _| {
            calls += 1;
            assert_eq!(candidate, &project);
            Ok::<_, Invalid>(())
        },
    )
    .unwrap();
    assert_eq!(calls, 1);
    assert!(!e.can_undo() && e.can_redo());
}
#[test]
fn valid_final_midi_batch_clears_route_after_removing_instrument() {
    let (project, destination) = midi_fixture();
    let mut e = Editor::new(project.clone(), 4).unwrap();
    assert!(
        e.execute(Command::RemoveInstrument { track: destination })
            .is_err()
    );
    let mut calls = 0;
    e.execute_many_prepared(
        vec![
            Command::RemoveInstrument { track: destination },
            Command::SetMidiSource {
                destination,
                source: None,
            },
        ],
        |candidate, results| {
            calls += 1;
            assert_eq!(results.len(), 2);
            assert!(candidate.midi_routes().is_empty());
            assert!(candidate.track(destination).unwrap().instrument().is_none());
            assert_eq!(
                Project::from_snapshot(candidate.snapshot()).unwrap(),
                *candidate
            );
            Ok::<_, Invalid>(())
        },
    )
    .unwrap();
    assert_eq!(calls, 1);
    let after = e.project().clone();
    assert!(e.undo());
    assert_eq!(e.project(), &project);
    assert!(!e.can_undo());
    assert!(e.redo());
    assert_eq!(e.project(), &after);
}
