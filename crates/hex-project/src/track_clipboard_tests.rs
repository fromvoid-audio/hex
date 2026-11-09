use super::*;
use crate::{BeatTime, Command, Editor, Tempo};

fn fixture() -> (Project, [TrackId; 3]) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported {
        track: a,
        clip,
        asset,
    } = e
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("audio");
    };
    e.execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let mut ids = vec![a];
    for name in ["midi", "empty"] {
        let Created::Track(id) = e
            .execute(Command::InsertTrackWithDevice {
                name: name.into(),
                before_track: None,
                slot: crate::TrackDeviceSlot::Instrument,
                state: crate::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        ids.push(id);
    }
    let [a, b, c] = [ids[0], ids[1], ids[2]];
    let Created::Clip(midi) = e
        .execute(Command::PlaceMidiClip {
            track: b,
            start: BeatTime::from_quarters(4).unwrap(),
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi");
    };
    e.execute(Command::AddMidiNote {
        clip: midi,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 64,
        channel: 2,
        velocity: 101,
    })
    .unwrap();
    let Created::Scene(scene) = e
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene");
    };
    e.execute(Command::CreateSessionAudioClip {
        track: a,
        scene,
        name: "session audio".into(),
        asset,
        source_start: 5,
        source_end: 100,
    })
    .unwrap();
    let Created::SessionClip(session) = e
        .execute(Command::CreateSessionMidiClip {
            track: b,
            scene,
            name: "session midi".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("session");
    };
    e.execute(Command::AddMidiNote {
        clip: session,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 67,
        channel: 0,
        velocity: 100,
    })
    .unwrap();
    e.execute(Command::SetSessionStopCell { track: c, scene })
        .unwrap();
    e.execute(Command::SetAudioFeed {
        source: RoutingOwner::Track(a),
        destination: b,
        gain: 0.5,
    })
    .unwrap();
    e.execute(Command::SetMidiSource {
        destination: b,
        source: Some(a),
    })
    .unwrap();
    e.execute(Command::SetMidiSource {
        destination: c,
        source: Some(b),
    })
    .unwrap();
    e.execute(Command::ReplaceTrackGainEnvelope {
        track: b,
        points: vec![crate::GainPoint {
            beat: BeatTime::ONE,
            gain: 0.5,
            incoming_gain: None,
            outgoing: crate::GainInterpolation::Step,
        }],
    })
    .unwrap();
    for value in [1, 2] {
        e.execute(Command::InsertDevice {
            track: b,
            before: None,
            state: hex_lattice_contract::NativeDeviceState::new(
                hex_lattice_contract::DeviceTypeId::from_static("test.effect"),
                1,
                vec![value],
            )
            .unwrap(),
        })
        .unwrap();
    }
    let Created::ReturnTrack(bus) = e
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    e.execute(Command::SetReturnSend {
        track: b,
        return_track: bus,
        gain: 0.25,
    })
    .unwrap();
    let mut p = e.project().clone();
    // Exercise complete metadata copying rather than just visible defaults.
    let device = p.track(b).unwrap().instrument().unwrap().id();
    p.edit_device(device, |instrument| {
        instrument.bypassed = true;
        instrument.comparison = Some(crate::DeviceComparison::new(
            crate::DeviceVariant::B,
            hex_lattice_contract::NativeDeviceState::new(
                instrument.state.type_id(),
                1,
                vec![1, 2, 3],
            )
            .unwrap()
            .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
                slot: hex_lattice_contract::DeviceAssetSlot::new(1).unwrap(),
                asset: hex_lattice_contract::DeviceAssetKey::new(asset.value()).unwrap(),
            }])
            .unwrap(),
        ));
        Ok(())
    })
    .unwrap();
    let t = p.track_mut(b).unwrap();
    t.gain = 0.37;
    t.balance = -0.25;
    t.muted = true;
    t.soloed = true;
    (p, [a, b, c])
}
fn normalize_copy(copy: &Track, original: &Track) -> Track {
    let mut t = copy.clone();
    t.id = original.id;
    for (d, old) in t.devices.iter_mut().zip(&original.devices) {
        d.id = old.id;
    }
    if let (Some(copy), Some(old)) = (t.instrument(), original.instrument()) {
        let mut copy = copy.clone();
        copy.id = old.id;
        assert_eq!(
            &copy, old,
            "explicit duplication retains complete source device state"
        );
    } else {
        assert_eq!(t.instrument.is_some(), original.instrument.is_some());
    }
    t.instrument = original.instrument.clone();
    for (clip, old) in t.content.audio_clips.iter_mut().zip(original.clips()) {
        clip.id = old.id;
    }
    for (clip, old) in t.content.midi_clips.iter_mut().zip(original.midi_clips()) {
        clip.id = old.id;
        for (note, old_note) in Arc::make_mut(&mut clip.source)
            .notes
            .iter_mut()
            .zip(&old.source.notes)
        {
            note.id = old_note.id;
        }
    }
    t
}

#[test]
fn serial_units_copy_with_fresh_internal_parents_and_one_history_step() {
    let (project, [_, track, _]) = fixture();
    let mut setup = Editor::new(project, 16).unwrap();
    let instrument = setup
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let mut ids = Vec::new();
    for index in 0..4 {
        let Created::Device(id) = setup
            .execute(Command::InsertDevice {
                track,
                before: (index < 3).then_some(instrument),
                state: crate::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("effect")
        };
        ids.push(id);
    }
    setup
        .execute(Command::ReplaceDeviceAutomation {
            device: ids[1],
            parameter: hex_lattice_contract::ParameterId::from_static(1),
            points: vec![crate::AutomationPoint {
                beat: BeatTime::ZERO,
                value: crate::AutomationValue::new(0.25).unwrap(),
                outgoing: crate::AutomationInterpolation::Linear,
            }],
        })
        .unwrap();
    let mut snapshot = setup.project().snapshot();
    let original = snapshot
        .tracks
        .iter_mut()
        .find(|item| item.id == track.value())
        .unwrap();
    original.devices[1].parent = Some(ids[0].value());
    original.devices[2].parent = Some(ids[1].value());
    original.devices[2].bypassed = true;
    original.devices[2].comparison = original.instrument.as_ref().unwrap().comparison.clone();
    let project = Project::from_snapshot(snapshot).unwrap();
    let selection = TrackSelectionIds::new(&[track]).unwrap();
    for without_clips in [false, true] {
        for duplicate in [false, true] {
            let command = if duplicate {
                Command::DuplicateTracks {
                    tracks: selection.clone(),
                    without_clips,
                }
            } else {
                Command::PasteTracks {
                    clipboard: Arc::new(project.capture_tracks(&selection, without_clips).unwrap()),
                    destination: crate::TrackDestination {
                        parent_group: None,
                        before: None,
                    },
                }
            };
            let mut editor = Editor::new(project.clone(), 8).unwrap();
            let refused: Result<_, Invalid> = editor.execute_prepared(command.clone(), |_| {
                Err::<(), _>(Invalid("prepare refused"))
            });
            assert_eq!(refused.unwrap_err(), Invalid("prepare refused"));
            assert_eq!(editor.project(), &project);
            assert!(!editor.can_undo());
            let Created::Tracks { first, .. } = editor.execute(command).unwrap() else {
                panic!("tracks")
            };
            let copy = editor.project().track(first).unwrap();
            let original = project.track(track).unwrap();
            assert_eq!(copy.instrument_position(), Some(3));
            assert_eq!(copy.devices().len(), original.devices().len());
            for (index, (copied, old)) in copy.devices().iter().zip(original.devices()).enumerate()
            {
                assert!(!ids.contains(&copied.id()));
                let expected_parent = match index {
                    1 => Some(copy.devices()[0].id()),
                    2 => Some(copy.devices()[1].id()),
                    _ => None,
                };
                assert_eq!(copied.parent(), expected_parent);
                let mut normalized = copied.clone();
                normalized.id = old.id();
                normalized.parent = old.parent();
                assert_eq!(&normalized, old);
            }
            assert_eq!(editor.project().track(track), Some(original));
            assert_eq!(copy.midi_clips().is_empty(), without_clips);
            let after = editor.project().snapshot();
            assert_eq!(
                Project::from_snapshot(after.clone()).unwrap(),
                *editor.project()
            );
            assert!(editor.undo());
            let mut before = project.snapshot();
            before.next_id = editor.project().snapshot().next_id;
            assert_eq!(editor.project().snapshot(), before);
            assert!(!editor.can_undo());
            assert!(editor.redo());
            assert_eq!(editor.project().snapshot(), after);
        }
    }
}
#[test]
fn full_tracks_remap_complete_state_routes_session_and_history_in_canonical_order() {
    let (p, [a, b, c]) = fixture();
    let clipboard = Arc::new(
        p.capture_tracks(&TrackSelectionIds::new(&[b, a]).unwrap(), false)
            .unwrap(),
    );
    let mut e = Editor::new(p.clone(), 8).unwrap();
    let Created::Tracks { first, last } = e
        .execute(Command::PasteTracks {
            clipboard,
            destination: crate::TrackDestination {
                parent_group: None,
                before: Some(c),
            },
        })
        .unwrap()
    else {
        panic!("tracks");
    };
    let copied = &e.project().tracks()[2..4];
    assert_eq!([copied[0].id(), copied[1].id()], [first, last]);
    for (t, old) in copied.iter().zip(&p.tracks()[..2]) {
        assert_eq!(normalize_copy(t, old), *old);
        assert_ne!(t.id(), old.id());
        for (d, o) in t
            .devices()
            .iter()
            .chain(t.instrument())
            .zip(old.devices().iter().chain(old.instrument()))
        {
            assert_ne!(d.id(), o.id());
        }
    }
    assert_ne!(
        copied[1].midi_clips()[0].source_identity(),
        p.track(b).unwrap().midi_clips()[0].source_identity()
    );
    assert!(Arc::ptr_eq(
        &copied[0].clips()[0].audio_edit().unwrap().domain,
        &p.track(a).unwrap().clips()[0].audio_edit().unwrap().domain
    ));
    assert!(
        e.project()
            .audio_feeds()
            .iter()
            .any(|r| r.source() == RoutingOwner::Track(first) && r.destination() == last)
    );
    assert_eq!(e.project().midi_source_track(last), first);
    assert_eq!(
        e.project().midi_source_track(c),
        a,
        "existing outside input is not replaced"
    );
    let cells: Vec<_> = e
        .project()
        .session_cells()
        .iter()
        .filter(|cell| [first, last].contains(&cell.track()))
        .collect();
    assert_eq!(cells.len(), 2);
    for cell in cells {
        let old = p
            .session_cells()
            .iter()
            .find(|old| old.track() == if cell.track() == first { a } else { b })
            .unwrap();
        let mut normalized = cell.clone();
        normalized.track = old.track;
        match (&mut normalized.content, &old.content) {
            (SessionCellContent::Audio(x), SessionCellContent::Audio(y)) => {
                assert_ne!(x.id, y.id);
                x.id = y.id;
            }
            (SessionCellContent::Midi(x), SessionCellContent::Midi(y)) => {
                assert_ne!(x.id, y.id);
                x.id = y.id;
                for (note, old) in Arc::make_mut(&mut x.source)
                    .notes
                    .iter_mut()
                    .zip(&y.source.notes)
                {
                    assert_ne!(note.id, old.id);
                    note.id = old.id;
                }
            }
            _ => panic!("content"),
        }
        assert_eq!(normalized, *old);
    }
    let after = e.project().clone();
    assert!(e.undo());
    let mut expected = p.snapshot();
    expected.next_id = e.project().snapshot().next_id;
    assert_eq!(e.project().snapshot(), expected);
    assert!(e.redo());
    assert_eq!(e.project(), &after);
}
#[test]
fn setup_only_copy_preserves_chain_mix_and_automation_without_any_clip_cells() {
    let (p, ids) = fixture();
    let clipboard = Arc::new(
        p.capture_tracks(&TrackSelectionIds::new(&ids).unwrap(), true)
            .unwrap(),
    );
    let mut e = Editor::new(p.clone(), 8).unwrap();
    e.execute(Command::PasteTracks {
        clipboard,
        destination: crate::TrackDestination {
            parent_group: None,
            before: None,
        },
    })
    .unwrap();
    for (t, old) in e.project().tracks()[3..].iter().zip(p.tracks()) {
        assert!(t.clips().is_empty() && t.midi_clips().is_empty());
        assert!(
            !e.project()
                .session_cells()
                .iter()
                .any(|c| c.track() == t.id())
        );
        let mut expected = old.clone();
        expected.content.audio_clips.clear();
        expected.content.midi_clips.clear();
        assert_eq!(normalize_copy(t, &expected), expected);
    }
    assert_eq!(&e.project().tracks()[..3], p.tracks());
}
#[test]
fn changed_tempo_reuses_onset_crop_law_and_failed_preparation_preserves_redo() {
    let (mut p, [a, _, _]) = fixture();
    let old = p.track(a).unwrap().clips()[0].clone();
    let mut second = old.clone();
    second.id = ClipId(p.allocate_id().unwrap());
    second.start = 48_000;
    p.track_mut(a).unwrap().content.audio_clips.push(second);
    let clipboard = Arc::new(
        p.capture_tracks(&TrackSelectionIds::new(&[a]).unwrap(), false)
            .unwrap(),
    );
    let mut e = Editor::new(p, 8).unwrap();
    e.execute(Command::SetProjectTempo {
        tempo: Tempo::new(240_000_000).unwrap(),
    })
    .unwrap();
    let command = Command::PasteTracks {
        clipboard,
        destination: crate::TrackDestination {
            parent_group: None,
            before: None,
        },
    };
    e.execute(command.clone()).unwrap();
    let clips = e.project().tracks().last().unwrap().clips();
    assert_eq!(
        (clips[0].start(), clips[0].duration(), clips[1].start()),
        (0, 24_000, 24_000)
    );
    assert!(e.undo());
    let before = e.project().snapshot();
    let refused: Result<_, Invalid> =
        e.execute_prepared(command, |_| Err::<(), _>(Invalid("publication refused")));
    assert!(refused.is_err());
    assert_eq!(e.project().snapshot(), before);
    assert!(e.can_redo());
}
#[test]
fn cut_restores_only_orphan_external_midi_and_stale_dependencies_refuse_atomically() {
    let (p, [a, b, c]) = fixture();
    let mut e = Editor::new(p.clone(), 8).unwrap();
    let selection = TrackSelectionIds::new(&[a, b]).unwrap();
    let clipboard = Arc::new(p.capture_tracks(&selection, false).unwrap());
    e.execute(Command::RemoveTracks(selection.ids().to_vec()))
        .unwrap();
    e.execute(Command::PasteTracks {
        clipboard: Arc::clone(&clipboard),
        destination: crate::TrackDestination {
            parent_group: None,
            before: None,
        },
    })
    .unwrap();
    assert_eq!(
        e.project().midi_source_track(c),
        e.project().tracks()[1].id()
    );
    e.execute(Command::PasteTracks {
        clipboard,
        destination: crate::TrackDestination {
            parent_group: None,
            before: None,
        },
    })
    .unwrap();
    assert_eq!(
        e.project().midi_source_track(c),
        e.project().tracks()[1].id(),
        "repeated paste preserves the restored external binding"
    );
    let mut e = Editor::new(p.clone(), 8).unwrap();
    let clipboard = Arc::new(
        p.capture_tracks(&TrackSelectionIds::new(&[b]).unwrap(), false)
            .unwrap(),
    );
    e.execute(Command::RemoveTrack(a)).unwrap();
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::PasteTracks {
            clipboard,
            destination: crate::TrackDestination {
                parent_group: None,
                before: None
            },
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    let mut full = p.clone();
    while full.tracks.len() < crate::MAX_TRACKS {
        let mut t = p.track(c).unwrap().clone();
        t.id = TrackId(full.allocate_id().unwrap());
        t.instrument = None;
        t.instrument_position = None;
        full.tracks.push(t);
    }
    let clipboard = Arc::new(
        p.capture_tracks(&TrackSelectionIds::new(&[b]).unwrap(), false)
            .unwrap(),
    );
    let mut e = Editor::new(full, 8).unwrap();
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::PasteTracks {
            clipboard,
            destination: crate::TrackDestination {
                parent_group: None,
                before: None
            },
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn setup_only_clipboard_checks_media_referenced_only_by_inactive_device_state() {
    let (mut p, [_, b, _]) = fixture();
    let selection = TrackSelectionIds::new(&[b]).unwrap();
    let clipboard = Arc::new(p.capture_tracks(&selection, true).unwrap());
    assert_eq!(clipboard.assets.len(), 1);
    let asset = clipboard.assets[0].0;
    p.assets.get_mut(&asset).unwrap().path = "relinked.wav".into();
    let mut e = Editor::new(p, 8).unwrap();
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::PasteTracks {
            clipboard,
            destination: crate::TrackDestination {
                parent_group: None,
                before: None
            },
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn grouped_track_clipboard_keeps_complete_children_and_setup_only_clears_content() {
    let (p, [a, b, c]) = fixture();
    for without_clips in [false, true] {
        let mut e = Editor::new(p.clone(), 16).unwrap();
        let Created::Track(group) = e
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[a, b]).unwrap(),
                name: "submix".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        let before = e.project().clone();
        let selection = TrackSelectionIds::new(&[group, a]).unwrap();
        let clipboard = Arc::new(
            e.project()
                .capture_tracks(&selection, without_clips)
                .unwrap(),
        );
        assert_eq!(clipboard.tracks.len(), 3);
        e.execute(Command::RemoveTracks(selection.ids().to_vec()))
            .unwrap();
        assert_eq!(
            e.project()
                .tracks()
                .iter()
                .map(Track::id)
                .collect::<Vec<_>>(),
            [c]
        );
        e.execute(Command::PasteTracks {
            clipboard,
            destination: crate::TrackDestination {
                parent_group: None,
                before: Some(c),
            },
        })
        .unwrap();
        let copied = &e.project().tracks()[..3];
        assert!(copied[0].is_group());
        for (copy, original) in copied.iter().zip(&before.tracks()[..3]) {
            assert_ne!(copy.id(), original.id());
            let mut normalized = normalize_copy(copy, original);
            normalized.parent_group = original.parent_group;
            let mut expected = original.clone();
            if without_clips {
                expected.content.audio_clips.clear();
                expected.content.midi_clips.clear();
            }
            assert_eq!(normalized, expected);
            if !copy.is_group() {
                assert_eq!(copy.parent_group(), Some(copied[0].id()));
            }
        }
        let copied_ids = [copied[1].id(), copied[2].id()];
        assert!(
            e.project()
                .audio_feeds()
                .iter()
                .any(|r| r.source() == RoutingOwner::Track(copied_ids[0])
                    && r.destination() == copied_ids[1])
        );
        assert_eq!(e.project().midi_source_track(copied_ids[1]), copied_ids[0]);
        assert_eq!(
            e.project()
                .session_cells()
                .iter()
                .filter(|cell| copied_ids.contains(&cell.track()))
                .count(),
            if without_clips { 0 } else { 2 }
        );
        assert_eq!(e.project().assets().count(), before.assets().count());
    }
}
