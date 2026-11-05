use hex_project::{
    BeatTime, Command, Created, Editor, GainInterpolation, GainPoint, Invalid, Project,
    RoutingOwner, TrackDeviceSlot, TrackId,
};

fn fixture() -> (Project, [TrackId; 3]) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { track: a, clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let mut instrument_track = |name: &str| {
        let Created::Track(id) = editor
            .execute(Command::InsertTrackWithDevice {
                name: name.into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("instrument track")
        };
        id
    };
    let b = instrument_track("instrument");
    let c = instrument_track("destination");
    let Created::Clip(midi) = editor
        .execute(Command::PlaceMidiClip {
            track: b,
            start: BeatTime::from_quarters(4).unwrap(),
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 64,
            channel: 0,
            velocity: 101,
        })
        .unwrap();
    editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track: a,
            points: vec![
                GainPoint {
                    beat: BeatTime::ZERO,
                    gain: 0.25,
                    incoming_gain: None,
                    outgoing: GainInterpolation::Linear,
                },
                GainPoint {
                    beat: BeatTime::ONE,
                    gain: 0.75,
                    incoming_gain: Some(0.5),
                    outgoing: GainInterpolation::Step,
                },
            ],
        })
        .unwrap();
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "return a".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    editor
        .execute(Command::AddReturnTrack {
            name: "return b".into(),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Track(a),
            destination: b,
            gain: 0.25,
        })
        .unwrap();
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
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    editor
        .execute(Command::CreateSessionMidiClip {
            track: b,
            scene,
            name: "session".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap();
    (editor.project().clone(), [a, b, c])
}

#[test]
fn moving_earlier_later_and_to_end_preserves_complete_id_owned_state_and_one_history_edge() {
    let (original, [a, b, c]) = fixture();
    let mut editor = Editor::new(original.clone(), 16).unwrap();
    for (track, before, order) in [
        (c, Some(a), [c, a, b]),
        (a, Some(c), [b, a, c]),
        (a, None, [b, c, a]),
    ] {
        let mut expected = original.snapshot();
        expected
            .tracks
            .sort_by_key(|row| order.iter().position(|id| id.value() == row.id).unwrap());
        let (_, prepared) = editor
            .execute_prepared(Command::MoveTrackBefore { track, before }, |candidate| {
                assert_eq!(candidate.snapshot(), expected);
                Ok::<_, Invalid>(candidate.snapshot())
            })
            .unwrap();
        assert_eq!(prepared, expected);
        assert_eq!(editor.project().snapshot(), expected);
        for id in [a, b, c] {
            let moved = editor.project().track(id).unwrap();
            let old = original.track(id).unwrap();
            assert_eq!(moved, old);
            for (left, right) in moved.clips().iter().zip(old.clips()) {
                assert!(std::sync::Arc::ptr_eq(
                    &left.audio_edit().unwrap().domain,
                    &right.audio_edit().unwrap().domain,
                ));
            }
            for (left, right) in moved.midi_clips().iter().zip(old.midi_clips()) {
                assert_eq!(left.source_identity(), right.source_identity());
            }
        }
        assert_eq!(editor.project().midi_source_track(c), a);
        assert!(editor.undo());
        assert_eq!(editor.project(), &original);
        assert!(!editor.can_undo());
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), expected);
        assert!(editor.undo());
        assert_eq!(editor.project(), &original);
    }
}

#[test]
fn self_adjacent_and_last_drops_keep_redo_and_do_not_add_history() {
    let (original, [a, b, c]) = fixture();
    let mut editor = Editor::new(original, 16).unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    for command in [
        Command::MoveTrackBefore {
            track: a,
            before: Some(a),
        },
        Command::MoveTrackBefore {
            track: a,
            before: Some(b),
        },
        Command::MoveTrackBefore {
            track: b,
            before: Some(c),
        },
        Command::MoveTrackBefore {
            track: c,
            before: None,
        },
    ] {
        editor.execute(command).unwrap();
        assert_eq!(editor.project(), &before);
        assert!(!editor.can_undo());
        assert!(editor.can_redo());
    }
    assert!(editor.redo());
    assert_eq!(editor.project().master_gain(), 0.5);
}

#[test]
fn stale_ids_and_failed_preparation_preserve_source_allocator_and_history() {
    let (original, [a, _, c]) = fixture();
    let mut editor = Editor::new(original, 16).unwrap();
    let Created::Track(stale) = editor
        .execute(Command::AddTrack {
            name: "undone".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    assert!(editor.undo());
    let before = editor.project().clone();
    for command in [
        Command::MoveTrackBefore {
            track: stale,
            before: Some(a),
        },
        Command::MoveTrackBefore {
            track: a,
            before: Some(stale),
        },
        Command::MoveTrackBefore {
            track: stale,
            before: Some(stale),
        },
    ] {
        let mut prepared = false;
        assert!(
            editor
                .execute_prepared::<_, Invalid>(command, |_| {
                    prepared = true;
                    Ok(())
                })
                .is_err()
        );
        assert!(!prepared);
        assert_eq!(editor.project(), &before);
        assert!(!editor.can_undo());
        assert!(editor.can_redo());
    }
    let refusal = editor.execute_prepared(
        Command::MoveTrackBefore {
            track: c,
            before: Some(a),
        },
        |candidate| {
            assert_eq!(candidate.tracks()[0].id(), c);
            Err::<(), _>(Invalid("prepared graph refused"))
        },
    );
    assert_eq!(refusal, Err(Invalid("prepared graph refused")));
    assert_eq!(editor.project(), &before);
    assert!(!editor.can_undo());
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert_eq!(editor.project().tracks().last().unwrap().id(), stale);
}

#[test]
fn selected_block_uses_canonical_member_order_and_preserves_complete_state_and_history() {
    let (original, [a, b, c]) = fixture();
    let mut setup = Editor::new(original, 16).unwrap();
    let Created::Track(d) = setup
        .execute(Command::AddTrack {
            name: "fourth".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let original = setup.project().clone();
    for (members, before, order) in [
        (vec![c, a], None, vec![b, d, a, c]),
        (vec![d, b], Some(a), vec![b, d, a, c]),
        (vec![c, b], Some(a), vec![b, c, a, d]),
        (vec![d, b], Some(c), vec![a, b, d, c]),
    ] {
        let mut editor = Editor::new(original.clone(), 16).unwrap();
        let mut expected = original.snapshot();
        expected
            .tracks
            .sort_by_key(|row| order.iter().position(|id| id.value() == row.id).unwrap());
        editor
            .execute_prepared(
                Command::MoveTracksBefore {
                    tracks: hex_project::TrackSelectionIds::new(&members).unwrap(),
                    before,
                },
                |candidate| {
                    assert_eq!(candidate.snapshot(), expected);
                    Ok::<_, Invalid>(())
                },
            )
            .unwrap();
        assert_eq!(editor.project().snapshot(), expected);
        for track in original.tracks() {
            let moved = editor.project().track(track.id()).unwrap();
            assert_eq!(moved, track);
            for (a, b) in moved.clips().iter().zip(track.clips()) {
                assert!(std::sync::Arc::ptr_eq(
                    &a.audio_edit().unwrap().domain,
                    &b.audio_edit().unwrap().domain
                ));
            }
            for (a, b) in moved.midi_clips().iter().zip(track.midi_clips()) {
                assert_eq!(a.source_identity(), b.source_identity());
            }
        }
        assert!(editor.undo());
        assert_eq!(editor.project(), &original);
        assert!(!editor.can_undo());
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), expected);
    }
}

#[test]
fn selected_targets_contiguous_boundaries_and_refused_preparation_preserve_redo() {
    let (original, [a, b, c]) = fixture();
    let mut editor = Editor::new(original.clone(), 16).unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    for (members, before) in [
        (vec![a, c], Some(a)),
        (vec![a, c], Some(c)),
        (vec![a, b], Some(c)),
        (vec![c, a, b], None),
    ] {
        editor
            .execute(Command::MoveTracksBefore {
                tracks: hex_project::TrackSelectionIds::new(&members).unwrap(),
                before,
            })
            .unwrap();
        assert_eq!(editor.project(), &original);
        assert!(!editor.can_undo());
        assert!(editor.can_redo());
    }
    let mut prepared = false;
    let error = editor
        .execute_prepared(
            Command::MoveTracksBefore {
                tracks: hex_project::TrackSelectionIds::new(&[c, a]).unwrap(),
                before: Some(b),
            },
            |candidate| {
                prepared = true;
                assert_eq!(
                    candidate
                        .tracks()
                        .iter()
                        .map(|t| t.id())
                        .collect::<Vec<_>>(),
                    [a, c, b]
                );
                Err::<(), _>(Invalid("injected preparation failure"))
            },
        )
        .unwrap_err();
    assert_eq!(error.0, "injected preparation failure");
    assert!(prepared);
    assert_eq!(editor.project(), &original);
    assert!(editor.can_redo());
    editor.execute(Command::RemoveTrack(c)).unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let stable = editor.project().clone();
    for (members, before) in [(vec![a, c], None), (vec![a, b], Some(c))] {
        let mut prepared = false;
        assert!(
            editor
                .execute_prepared(
                    Command::MoveTracksBefore {
                        tracks: hex_project::TrackSelectionIds::new(&members).unwrap(),
                        before,
                    },
                    |_| {
                        prepared = true;
                        Ok::<_, Invalid>(())
                    }
                )
                .is_err()
        );
        assert!(!prepared);
        assert_eq!(editor.project(), &stable);
        assert!(editor.can_redo());
    }
}

#[test]
fn captured_track_membership_is_nonempty_unique_and_bounded_before_queueing() {
    // Obtain unique typed identities without building a large musical fixture.
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let mut ids = Vec::new();
    for _ in 0..=hex_project::MAX_TRACKS {
        let Created::Track(id) = editor
            .execute(Command::AddTrack {
                name: "identity".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        ids.push(id);
        editor.execute(Command::RemoveTrack(id)).unwrap();
    }
    assert!(hex_project::TrackSelectionIds::new(&[]).is_err());
    assert!(hex_project::TrackSelectionIds::new(&[ids[0], ids[0]]).is_err());
    let maximum = hex_project::TrackSelectionIds::new(&ids[..hex_project::MAX_TRACKS]).unwrap();
    assert_eq!(maximum.ids(), &ids[..hex_project::MAX_TRACKS]);
    assert!(hex_project::TrackSelectionIds::new(&ids).is_err());
}
