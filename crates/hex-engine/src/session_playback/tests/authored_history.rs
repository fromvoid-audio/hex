//! Ordinary complete authored membership authorizes suppression, never manufactured DSP history.
use super::*;

fn registry_test(
    run: impl FnOnce(&hex_lattice::NativeInstrumentRegistry<'_>, hex_lattice_contract::DeviceTypeId),
) {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.authored-history");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    run(&registry, id);
}
fn arrangement(snapshot: &mut Snapshot) -> &mut Vec<MidiClipSnapshot> {
    match &mut snapshot.tracks[0].content {
        TrackContentSnapshot::Instrument(clips)
        | TrackContentSnapshot::Hybrid {
            midi_clips: clips, ..
        } => clips,
        _ => panic!("fixture MIDI track"),
    }
}
fn session(snapshot: &mut Snapshot) -> &mut MidiSourceSnapshot {
    let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content else {
        panic!("fixture MIDI cell")
    };
    &mut clip.source
}
fn warm(old: &mut SessionPlayback, selected_session: bool) {
    old.apply(SessionPlaybackCommand::Play).unwrap();
    if selected_session {
        old.apply(SessionPlaybackCommand::LaunchClip {
            clip: old.revision.session().midi_clips()[0].id,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    }
    old.render(&mut [[0.0; 2]; 64]).unwrap();
}

#[test]
fn ordinary_restored_or_added_authored_notes_suppress_past_and_admit_future_atomically() {
    registry_test(|registry, id| {
        let full = with_test_instrument(
            &with_second_arrangement_note(&with_second_session_note(&project(1.0, 0, 8 << 32))),
            id,
            0,
        );
        for selected_session in [false, true] {
            let mut old_snapshot = full.snapshot();
            arrangement(&mut old_snapshot)[0].notes.remove(0);
            session(&mut old_snapshot).notes.remove(0);
            let before = Project::from_snapshot(old_snapshot).unwrap();
            let mut old = ordinary_mutable_playback(&before, registry);
            warm(&mut old, selected_session);
            let owner = old.owner(full.tracks()[0].id());
            let mut next = ordinary_mutable_playback(&full, registry);
            next.release_capacity = 0;
            assert_eq!(
                next.try_inherit_state_from(&mut old),
                Err(SessionPlaybackError::ReleaseBankFull)
            );
            assert_eq!(old.performance_frame(), 64);
            assert_eq!(old.owner(full.tracks()[0].id()), owner);
            assert!(old.suppressions.is_empty());
            assert!(next.suppressions.is_empty());
            next.release_capacity = SESSION_RELEASE_CAPACITY;
            next.try_inherit_state_from(&mut old).unwrap();
            assert_eq!(next.suppressions.len(), 1);
            let mut silent = [[1.0; 2]; 64];
            next.render(&mut silent).unwrap();
            assert_eq!(silent, [[0.0; 2]; 64]);
            let mut successor = ordinary_mutable_playback(&full, registry);
            successor.try_inherit_state_from(&mut next).unwrap();
            successor.render(&mut silent).unwrap();
            assert_eq!(silent, [[0.0; 2]; 64]);
            let mut until_onset = vec![[0.0; 2]; 24_000 - 192];
            successor.render(&mut until_onset).unwrap();
            assert!(until_onset.iter().all(|frame| *frame == [0.0; 2]));
            successor.render(&mut silent[..1]).unwrap();
            assert!(silent[0][0] > 0.0, "exact future onset remains playable");
            // A legacy graph cannot prove absence, even when its authoring appears equivalent.
            let mut legacy = SessionPlayback::new(revision_with_instrument_registry(
                &before,
                Arc::new(hex_devices::SineTable::new()),
                registry,
            ))
            .unwrap();
            warm(&mut legacy, selected_session);
            assert_eq!(
                ordinary_mutable_playback(&full, registry).try_inherit_state_from(&mut legacy),
                Err(SessionPlaybackError::MutableInstrumentChase)
            );
        }
        // An entirely absent old Arrangement clip is complete evidence too.
        let mut absent = full.snapshot();
        arrangement(&mut absent).clear();
        let mut old = ordinary_mutable_playback(&Project::from_snapshot(absent).unwrap(), registry);
        warm(&mut old, false);
        let mut restored = ordinary_mutable_playback(&full, registry);
        restored.try_inherit_state_from(&mut old).unwrap();
        assert_eq!(restored.suppressions.len(), 1);

        // Suppression belongs to one occurrence, not later Session iterations or transport passes.
        for transport_loop in [false, true] {
            let mut looped = with_test_instrument(&project(1.0, 0, 1 << 30), id, 0).snapshot();
            if transport_loop {
                looped.loop_region = Some(LoopRegion::new(0, 24_000, true).unwrap());
            } else {
                session(&mut looped).looping = true;
                session(&mut looped).loop_end_raw = 1 << 32;
            }
            let full = Project::from_snapshot(looped.clone()).unwrap();
            if transport_loop {
                arrangement(&mut looped)[0].notes.clear();
            } else {
                session(&mut looped).notes.clear();
            }
            let mut old =
                ordinary_mutable_playback(&Project::from_snapshot(looped).unwrap(), registry);
            warm(&mut old, !transport_loop);
            let mut added = ordinary_mutable_playback(&full, registry);
            added.try_inherit_state_from(&mut old).unwrap();
            let mut first_iteration = vec![[0.0; 2]; 24_000 - 64];
            added.render(&mut first_iteration).unwrap();
            assert!(first_iteration.iter().all(|frame| *frame == [0.0; 2]));
            let mut repeated = [[0.0; 2]; 1];
            added.render(&mut repeated).unwrap();
            assert!(repeated[0][0] > 0.0, "next loop iteration is a new onset");
        }
    });
}

#[test]
fn ordinary_existing_authored_identity_marker_or_owner_changes_remain_strict() {
    registry_test(|registry, id| {
        for selected_session in [false, true] {
            for case in 0..4 {
                let full = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);
                let mut before = full.snapshot();
                let mut after = full.snapshot();
                match case {
                    0 => {
                        // Existing ID moved from a future onset into the past.
                        arrangement(&mut before)[0].notes[0].start_raw = 1 << 32;
                        session(&mut before).notes[0].start_raw = 1 << 32;
                    }
                    1 => {
                        // Conflicting signature of a live dispatched stable key.
                        arrangement(&mut after)[0].notes[0].pitch += 1;
                        session(&mut after).notes[0].pitch += 1;
                    }
                    2 => {
                        // Old authored ID is outside playable markers, yet still exists.
                        arrangement(&mut before)[0].notes[0].start_raw = 20 << 32;
                        session(&mut before).notes[0].start_raw = 20 << 32;
                        for snapshot in [&mut before, &mut after] {
                            let source = session(snapshot);
                            source.looping = true;
                            source.loop_end_raw = 1 << 32;
                        }
                        arrangement(&mut before)[0].notes[0].duration_raw = 1 << 32;
                        session(&mut before).notes[0].duration_raw = 1 << 32;
                    }
                    _ => {
                        // Newly added ID does not waive changed playback markers.
                        arrangement(&mut before)[0].notes.clear();
                        session(&mut before).notes.clear();
                        arrangement(&mut after)[0].loop_end_raw = 31 << 32;
                        session(&mut after).loop_end_raw = 31 << 32;
                    }
                }
                let mut old =
                    ordinary_mutable_playback(&Project::from_snapshot(before).unwrap(), registry);
                warm(&mut old, selected_session);
                let mut next =
                    ordinary_mutable_playback(&Project::from_snapshot(after).unwrap(), registry);
                assert_eq!(
                    next.try_inherit_state_from(&mut old),
                    Err(SessionPlaybackError::MutableInstrumentChase),
                    "session={selected_session} case={case}"
                );
                assert_eq!(old.performance_frame(), 64);
                assert_eq!(next.performance_frame(), 0);
                assert!(next.suppressions.is_empty());
            }
        }
        // Moving a globally existing clip to an already existing different track is not new.
        let full = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);
        let mut before = full.snapshot();
        let mut second = before.tracks[0].clone();
        second.id = before.next_id;
        before.next_id += 1;
        second.instrument.as_mut().unwrap().id = before.next_id;
        before.next_id += 1;
        second.content = TrackContentSnapshot::Instrument(Vec::new());
        before.tracks.push(second);
        let mut after = before.clone();
        after.tracks[1].content = std::mem::replace(
            &mut after.tracks[0].content,
            TrackContentSnapshot::Instrument(Vec::new()),
        );
        let mut old = ordinary_mutable_playback(&Project::from_snapshot(before).unwrap(), registry);
        warm(&mut old, false);
        let mut next = ordinary_mutable_playback(&Project::from_snapshot(after).unwrap(), registry);
        assert_eq!(
            next.try_inherit_state_from(&mut old),
            Err(SessionPlaybackError::MutableInstrumentChase)
        );
        assert_eq!(old.performance_frame(), 64);
    });
}
