use hex_project::{
    AudioFeedSnapshot, Command, Created, Editor, MAX_AUDIO_FEEDS, MidiRouteSnapshot, Project,
    RoutingOwner, TrackId,
};

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
        panic!("track");
    };
    id
}

#[test]
fn audio_inputs_validate_the_combined_graph_and_history_without_reusing_ids() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let a = track(&mut editor);
    let b = track(&mut editor);
    let c = track(&mut editor);
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack { name: "bus".into() })
        .unwrap()
    else {
        panic!("return");
    };
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Track(a),
            destination: b,
            gain: 0.25,
        })
        .unwrap();
    let id = editor.project().audio_feeds()[0].id();
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Track(a),
            destination: b,
            gain: 2.0,
        })
        .unwrap();
    assert_eq!(editor.project().audio_feeds()[0].id(), id);
    editor
        .execute(Command::SetReturnSend {
            track: b,
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Return(bus),
            destination: c,
            gain: 0.75,
        })
        .unwrap();
    let before = editor.project().clone();
    for (source, destination, gain) in [
        (RoutingOwner::Return(bus), a, 1.0),
        (RoutingOwner::Track(a), a, 1.0),
        (RoutingOwner::Main, b, 1.0),
        (RoutingOwner::Track(a), b, f32::NAN),
        (RoutingOwner::Track(a), b, 2.1),
    ] {
        assert!(
            editor
                .execute(Command::SetAudioFeed {
                    source,
                    destination,
                    gain
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
    }
    // Adding a send after a legal Return input must also revalidate the graph.
    assert!(
        editor
            .execute(Command::SetReturnSend {
                track: c,
                return_track: bus,
                gain: 1.0
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Track(a),
            destination: b,
            gain: 0.0,
        })
        .unwrap();
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Track(a),
            destination: b,
            gain: 0.5,
        })
        .unwrap();
    assert!(
        editor
            .project()
            .audio_feeds()
            .iter()
            .find(|f| f.source() == RoutingOwner::Track(a))
            .unwrap()
            .id()
            .value()
            > id.value()
    );
    let with_routes = editor.project().clone();
    editor.execute(Command::RemoveTrack(b)).unwrap();
    assert_eq!(editor.project().audio_feeds().len(), 1);
    assert!(editor.undo());
    assert_eq!(editor.project(), &with_routes);
    editor.execute(Command::RemoveReturnTrack(bus)).unwrap();
    assert_eq!(editor.project().audio_feeds().len(), 1);
    assert!(editor.project().track(b).unwrap().sends().is_empty());
}

#[test]
fn midi_bus_selection_supports_source_only_tracks_root_resolution_and_fanout() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let a = track(&mut editor);
    let b = track(&mut editor);
    let c = track(&mut editor);
    let d = track(&mut editor);
    editor
        .execute(Command::RemoveInstrument { track: a })
        .unwrap();
    editor
        .execute(Command::SetMidiSource {
            destination: b,
            source: Some(a),
        })
        .unwrap();
    editor
        .execute(Command::SetMidiSource {
            destination: c,
            source: Some(b),
        })
        .unwrap();
    editor
        .execute(Command::SetMidiSource {
            destination: d,
            source: Some(a),
        })
        .unwrap();
    assert_eq!(editor.project().midi_source_track(c), a);
    assert_eq!(editor.project().midi_source_track(d), a);
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::RemoveInstrument { track: b })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(
        editor
            .execute(Command::SetMidiSource {
                destination: b,
                source: Some(c)
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(
        editor
            .execute(Command::SetMidiSource {
                destination: a,
                source: Some(b)
            })
            .is_err()
    );
    let id = editor.project().midi_routes()[0].id();
    editor
        .execute(Command::SetMidiSource {
            destination: b,
            source: Some(d),
        })
        .unwrap();
    assert_eq!(editor.project().midi_routes()[0].id(), id);
    editor
        .execute(Command::SetMidiSource {
            destination: b,
            source: None,
        })
        .unwrap();
    assert_eq!(editor.project().midi_source_track(b), b);
    assert!(editor.undo());
    assert_eq!(editor.project().midi_source_track(c), a);
    let with_routes = editor.project().clone();
    editor.execute(Command::RemoveTrack(a)).unwrap();
    assert_eq!(editor.project().midi_routes().len(), 2);
    assert_eq!(editor.project().midi_source_track(c), d);
    assert!(editor.undo());
    assert_eq!(editor.project(), &with_routes);
    assert_eq!(
        Project::from_snapshot(with_routes.snapshot()).unwrap(),
        with_routes
    );
}

#[test]
fn snapshot_routes_refuse_colliding_ids_duplicates_and_excess_without_truncation() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let ids: Vec<_> = (0..96).map(|_| track(&mut editor)).collect();
    let mut snapshot = editor.project().snapshot();
    'full: for (index, &source) in ids.iter().enumerate() {
        for &destination in &ids[index + 1..] {
            if snapshot.audio_feeds.len() == MAX_AUDIO_FEEDS {
                break 'full;
            }
            snapshot.audio_feeds.push(AudioFeedSnapshot {
                id: snapshot.next_id,
                source: RoutingOwner::Track(source),
                destination: destination.value(),
                gain: 1.0,
            });
            snapshot.next_id += 1;
        }
    }
    assert_eq!(snapshot.audio_feeds.len(), MAX_AUDIO_FEEDS);
    let maximum = Project::from_snapshot(snapshot.clone()).unwrap();
    assert_eq!(maximum.audio_feeds().len(), MAX_AUDIO_FEEDS);
    let mut invalid = snapshot.clone();
    invalid.audio_feeds.push(AudioFeedSnapshot {
        id: invalid.next_id,
        source: RoutingOwner::Track(ids[94]),
        destination: ids[95].value(),
        gain: 1.0,
    });
    invalid.next_id += 1;
    assert!(Project::from_snapshot(invalid).is_err());
    for id in [0, ids[0].value(), snapshot.next_id] {
        let mut invalid = snapshot.clone();
        invalid.audio_feeds[0].id = id;
        assert!(Project::from_snapshot(invalid).is_err());
    }
    let mut invalid = snapshot.clone();
    invalid.audio_feeds[1].source = invalid.audio_feeds[0].source;
    invalid.audio_feeds[1].destination = invalid.audio_feeds[0].destination;
    assert!(Project::from_snapshot(invalid).is_err());
    let mut invalid = snapshot;
    invalid.midi_routes.push(MidiRouteSnapshot {
        id: invalid.audio_feeds[0].id,
        source: ids[0].value(),
        destination: ids[1].value(),
    });
    assert!(Project::from_snapshot(invalid).is_err());
}

#[test]
fn return_feed_mutation_checks_feedback_through_track_input_and_send() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let a = track(&mut editor);
    let Created::ReturnTrack(x) = editor
        .execute(Command::AddReturnTrack { name: "x".into() })
        .unwrap()
    else {
        panic!("return");
    };
    let Created::ReturnTrack(y) = editor
        .execute(Command::AddReturnTrack { name: "y".into() })
        .unwrap()
    else {
        panic!("return");
    };
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Return(y),
            destination: a,
            gain: 1.0,
        })
        .unwrap();
    editor
        .execute(Command::SetReturnSend {
            track: a,
            return_track: x,
            gain: 1.0,
        })
        .unwrap();
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::SetReturnFeed {
                source: x,
                destination: y,
                gain: 1.0
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(before.snapshot().with_return_feed(x, y, 1.0).is_err());
}

#[test]
fn compressor_sidechain_uses_combined_cycle_checks_and_history() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let source = track(&mut editor);
    let receiver = track(&mut editor);
    let state = hex_lattice_contract::NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::from_static("hex.compressor"),
        3,
        vec![0; 32],
    )
    .unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track: receiver,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("compressor")
    };
    let original = editor.project().clone();
    let connect = Command::SetSidechain {
        device,
        source: Some(source),
    };
    editor.execute(connect.clone()).unwrap();
    let connected = editor.project().clone();
    assert_eq!(
        connected.sidechains(),
        &[hex_project::SidechainRoute { source, device }]
    );
    assert!(editor.undo());
    assert_eq!(editor.project(), &original);
    assert!(editor.redo());
    assert_eq!(editor.project(), &connected);

    for command in [
        Command::SetSidechain {
            device,
            source: Some(receiver),
        },
        Command::SetAudioFeed {
            source: RoutingOwner::Track(receiver),
            destination: source,
            gain: 1.0,
        },
    ] {
        assert!(editor.execute(command).is_err());
        assert_eq!(editor.project(), &connected);
    }

    editor.execute(Command::RemoveTrack(source)).unwrap();
    assert!(editor.project().sidechains().is_empty());
    assert!(editor.execute(connect).is_err());
    assert!(editor.undo());
    assert_eq!(editor.project(), &connected);
    editor.execute(Command::RemoveDevice(device)).unwrap();
    assert!(editor.project().sidechains().is_empty());
    assert!(editor.undo());
    assert_eq!(editor.project(), &connected);
}
