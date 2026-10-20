use super::*;
use hex_project::{Command, Created, Editor, ReturnId, TrackId};
fn track(editor: &mut Editor, midi: bool) -> TrackId {
    let command = if midi {
        Command::InsertTrackWithDevice {
            name: "midi".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }
    } else {
        Command::AddTrack {
            name: "audio".into(),
        }
    };
    let Created::Track(id) = editor.execute(command).unwrap() else {
        panic!("track")
    };
    id
}
fn bus(editor: &mut Editor) -> ReturnId {
    let Created::ReturnTrack(id) = editor
        .execute(Command::AddReturnTrack { name: "bus".into() })
        .unwrap()
    else {
        panic!("bus")
    };
    id
}
#[test]
fn borrowed_routing_rejects_actual_prepared_batch_combined_audio_cycle() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let t = track(&mut editor, false);
    let a = bus(&mut editor);
    let b = bus(&mut editor);
    editor
        .execute(Command::SetReturnFeed {
            source: a,
            destination: b,
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Return(b),
            destination: t,
            gain: 0.5,
        })
        .unwrap();
    validate(editor.project()).unwrap();
    Project::validate_return_feed_snapshot(&editor.project().snapshot()).unwrap();
    let before = editor.project().snapshot();
    let history = (editor.can_undo(), editor.can_redo());
    let mut prepared = false;
    let error = editor
        .execute_many_prepared(
            vec![Command::SetReturnSend {
                track: t,
                return_track: a,
                gain: 0.5,
            }],
            |_, _| {
                prepared = true;
                Ok::<_, hex_project::Invalid>(())
            },
        )
        .unwrap_err();
    assert_eq!(
        error,
        hex_project::Invalid("routing cycles are unsupported")
    );
    assert!(!prepared);
    assert_eq!(editor.project().snapshot(), before);
    assert_eq!((editor.can_undo(), editor.can_redo()), history);
    validate(editor.project()).unwrap();
    let mut invalid = before;
    invalid
        .tracks
        .iter_mut()
        .find(|row| row.id == t.value())
        .unwrap()
        .sends
        .push(hex_project::SendLevelSnapshot {
            return_track: a.value(),
            gain: 0.5,
        });
    assert_eq!(
        Project::validate_return_feed_snapshot(&invalid),
        Err(hex_project::Invalid("routing cycles are unsupported"))
    );
}
#[test]
fn borrowed_routing_rejects_actual_prepared_batch_missing_midi_instrument() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let source = track(&mut editor, true);
    let destination = track(&mut editor, true);
    editor
        .execute(Command::SetMidiSource {
            source: Some(source),
            destination,
        })
        .unwrap();
    validate(editor.project()).unwrap();
    let before = editor.project().snapshot();
    let history = (editor.can_undo(), editor.can_redo());
    let mut prepared = false;
    let error = editor
        .execute_many_prepared(
            vec![Command::RemoveInstrument { track: destination }],
            |_, _| {
                prepared = true;
                Ok::<_, hex_project::Invalid>(())
            },
        )
        .unwrap_err();
    assert_eq!(
        error,
        hex_project::Invalid("midi destination requires an instrument")
    );
    assert!(!prepared);
    assert_eq!(editor.project().snapshot(), before);
    assert_eq!((editor.can_undo(), editor.can_redo()), history);
    validate(editor.project()).unwrap();
    let mut invalid = before;
    let row = invalid
        .tracks
        .iter_mut()
        .find(|row| row.id == destination.value())
        .unwrap();
    row.instrument = None;
    row.instrument_position = None;
    assert_eq!(
        Project::validate_return_feed_snapshot(&invalid),
        Err(hex_project::Invalid(
            "midi destination requires an instrument"
        ))
    );
}
#[test]
fn borrowed_routing_scratch_refuses_before_construction_and_counts_actual_capacities() {
    let project = Project::new(48000).unwrap();
    let counts = Counts::of(&project).unwrap();
    let expected =
        size_of::<Scratch>() + size_of::<RoutingOwner>() + size_of::<usize>() + size_of::<bool>();
    assert_eq!(counts.bytes().unwrap(), expected);
    assert!(Scratch::new(counts, expected - 1).is_err());
    let scratch = Scratch::new(counts, expected).unwrap();
    assert_eq!(scratch.nodes.capacity(), 1);
    assert_eq!(scratch.incoming.capacity(), 1);
    assert_eq!(scratch.visited.capacity(), 1);
    assert_eq!(scratch.audio.capacity(), 0);
    validate_reserved(&project, counts, expected).unwrap();
}
#[test]
fn borrowed_routing_matches_strict_complete_shuffled_return_dag() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 1).unwrap();
    let ids: Vec<_> = (0..hex_project::MAX_RETURN_TRACKS)
        .map(|_| bus(&mut editor))
        .collect();
    let mut snapshot = editor.project().snapshot();
    for (i, &source) in ids.iter().enumerate() {
        for &destination in &ids[i + 1..] {
            snapshot = snapshot.with_return_feed(source, destination, 0.5).unwrap();
        }
    }
    snapshot.return_feeds.reverse();
    Project::validate_return_feed_snapshot(&snapshot).unwrap();
    let project = Project::from_snapshot(snapshot).unwrap();
    validate(&project).unwrap();
    assert_eq!(Counts::of(&project).unwrap().audio, 16 + 120);
}

#[test]
fn borrowed_routing_combined_edges_above_authored_limit_remain_valid() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 1).unwrap();
    // Construct the large fixture once through the public snapshot ingress.
    let commands = (0..hex_project::MAX_TRACKS)
        .map(|_| Command::AddTrack {
            name: "track".into(),
        })
        .chain(
            (0..hex_project::MAX_RETURN_TRACKS)
                .map(|_| Command::AddReturnTrack { name: "bus".into() }),
        )
        .collect();
    editor
        .execute_many_prepared(commands, |_, _| Ok::<_, hex_project::Invalid>(()))
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    for track in &mut snapshot.tracks {
        track.muted = true; // muted owners still contribute all structural edges
        track.sends = snapshot
            .return_tracks
            .iter()
            .map(|bus| hex_project::SendLevelSnapshot {
                return_track: bus.id,
                gain: 0.5,
            })
            .collect();
    }
    Project::validate_return_feed_snapshot(&snapshot).unwrap();
    let project = Project::from_snapshot(snapshot).unwrap();
    let counts = Counts::of(&project).unwrap();
    assert_eq!(counts.nodes, 273);
    assert_eq!(counts.audio, 256 * 16 + 256 + 16);
    assert!(counts.audio > hex_project::MAX_AUTHORED_ROUTES);
    validate(&project).unwrap();
    assert!(validate_reserved(&project, counts, counts.bytes().unwrap() - 1).is_err());
}
