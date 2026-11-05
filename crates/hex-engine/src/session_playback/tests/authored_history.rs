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
fn make_alias_unique_and_undo_preserve_held_voice_history_and_note_off() {
    registry_test(|registry, id| {
        for selected_session in [false, true] {
            let base = with_test_instrument(&project(1.0, 0, 1 << 26), id, 0);
            let mut editor = Editor::new(base, 16).unwrap();
            let original = if selected_session {
                let cell = &editor.project().session_cells()[0];
                let (track, scene) = (cell.track(), cell.scene());
                let hex_project::SessionCellContent::Midi(clip) = cell.content() else {
                    panic!("midi cell")
                };
                let original = clip.id();
                editor
                    .execute(Command::DuplicateSessionCell {
                        track,
                        scene,
                        alias: true,
                    })
                    .unwrap();
                original
            } else {
                let original = editor.project().tracks()[0].midi_clips()[0].id();
                editor
                    .execute(Command::DuplicateAliasClips(vec![original]))
                    .unwrap();
                original
            };
            let mut active = ordinary_mutable_playback(editor.project(), registry);
            let mut reference = ordinary_mutable_playback(editor.project(), registry);
            warm(&mut active, selected_session);
            warm(&mut reference, selected_session);
            editor
                .execute(Command::MakeClipsUnique(vec![original]))
                .unwrap();
            let mut unique = ordinary_mutable_playback(editor.project(), registry);
            unique.try_inherit_state_from(&mut active).unwrap();
            let mut actual = [[0.0; 2]; 127];
            let mut expected = actual;
            unique.render(&mut actual).unwrap();
            reference.render(&mut expected).unwrap();
            assert_eq!(actual, expected);
            assert!(actual.iter().flatten().any(|sample| sample.abs() > 0.01));
            assert!(editor.undo());
            let mut restored = ordinary_mutable_playback(editor.project(), registry);
            restored.try_inherit_state_from(&mut unique).unwrap();
            let mut actual = vec![[0.0; 2]; 2_048];
            let mut expected = actual.clone();
            for chunk in actual.chunks_mut(17) {
                restored.render(chunk).unwrap();
            }
            reference.render(&mut expected).unwrap();
            assert_eq!(actual, expected);
            assert!(actual[1_984..].iter().all(|frame| *frame == [0.0; 2]));
        }
    });
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
            let before = Project::from_legacy_snapshot(old_snapshot).unwrap();
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
        let mut old =
            ordinary_mutable_playback(&Project::from_legacy_snapshot(absent).unwrap(), registry);
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
            let full = Project::from_legacy_snapshot(looped.clone()).unwrap();
            if transport_loop {
                arrangement(&mut looped)[0].notes.clear();
            } else {
                session(&mut looped).notes.clear();
            }
            let mut old = ordinary_mutable_playback(
                &Project::from_legacy_snapshot(looped).unwrap(),
                registry,
            );
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
fn newly_enabled_notes_do_not_chase_missing_mutable_history() {
    registry_test(|registry, id| {
        let full = with_test_instrument(
            &with_second_arrangement_note(&with_second_session_note(&project(1.0, 0, 8 << 32))),
            id,
            0,
        );
        for selected_session in [false, true] {
            let mut snapshot = full.snapshot();
            arrangement(&mut snapshot)[0].notes[0].active = false;
            session(&mut snapshot).notes[0].active = false;
            let before = Project::from_legacy_snapshot(snapshot).unwrap();
            let mut old = ordinary_mutable_playback(&before, registry);
            warm(&mut old, selected_session);
            let mut next = ordinary_mutable_playback(&full, registry);
            next.try_inherit_state_from(&mut old).unwrap();
            assert_eq!(next.suppressions.len(), 1);
            let mut output = [[1.0; 2]; 64];
            next.render(&mut output).unwrap();
            assert_eq!(output, [[0.0; 2]; 64]);
            let mut until_onset = vec![[0.0; 2]; 24_000 - 128];
            next.render(&mut until_onset).unwrap();
            assert!(until_onset.iter().all(|frame| *frame == [0.0; 2]));
            next.render(&mut output[..1]).unwrap();
            assert!(output[0][0] > 0.0);
            let mut legacy = SessionPlayback::new(revision_with_instrument_registry(
                &before,
                Arc::new(hex_devices::SineTable::new()),
                registry,
            ))
            .unwrap();
            warm(&mut legacy, selected_session);
            let mut legacy_next = SessionPlayback::new(revision_with_instrument_registry(
                &full,
                Arc::new(hex_devices::SineTable::new()),
                registry,
            ))
            .unwrap();
            assert_eq!(
                legacy_next.try_inherit_state_from(&mut legacy),
                Err(SessionPlaybackError::MutableInstrumentChase)
            );
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
                let mut old = ordinary_mutable_playback(
                    &Project::from_legacy_snapshot(before).unwrap(),
                    registry,
                );
                warm(&mut old, selected_session);
                let mut next = ordinary_mutable_playback(
                    &Project::from_legacy_snapshot(after).unwrap(),
                    registry,
                );
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
        let mut old =
            ordinary_mutable_playback(&Project::from_legacy_snapshot(before).unwrap(), registry);
        warm(&mut old, false);
        let mut next =
            ordinary_mutable_playback(&Project::from_legacy_snapshot(after).unwrap(), registry);
        assert_eq!(
            next.try_inherit_state_from(&mut old),
            Err(SessionPlaybackError::MutableInstrumentChase)
        );
        assert_eq!(old.performance_frame(), 64);
    });
}

#[test]
fn section_duplicate_undo_restores_prefix_without_chasing_a_past_gate() {
    registry_test(|registry, id| {
        for (looped, shared) in [(false, false), (true, false), (false, true), (true, true)] {
            for elapsed in [64, 48_064] {
                let mut snapshot =
                    with_test_instrument(&project(1.0, 0, 6 << 32), id, 0).snapshot();
                if looped {
                    snapshot.loop_region = Some(LoopRegion::new(0, 48_000, true).unwrap());
                }
                let mut editor =
                    Editor::new(Project::from_legacy_snapshot(snapshot).unwrap(), 8).unwrap();
                if shared {
                    let source = editor.project().tracks()[0].source_id().unwrap();
                    let Created::Track(peer) = editor
                        .execute(Command::AddMidiTrack {
                            name: "peer".into(),
                        })
                        .unwrap()
                    else {
                        panic!("peer")
                    };
                    editor
                        .execute(Command::BindTrackSource {
                            track: peer,
                            source: Some(source),
                        })
                        .unwrap();
                    let Created::Clip(clip) = editor
                        .execute(Command::PlaceMidiClip {
                            track: peer,
                            start: BeatTime::ZERO,
                            length: BeatTime::from_quarters(8).unwrap(),
                        })
                        .unwrap()
                    else {
                        panic!("clip")
                    };
                    editor
                        .execute(Command::AddMidiNote {
                            clip,
                            start: BeatTime::ZERO,
                            duration: BeatTime::from_quarters(8).unwrap(),
                            pitch: 67,
                            channel: 0,
                            velocity: 100,
                        })
                        .unwrap();
                }
                let Created::ArrangementSection(section) = editor
                    .execute(Command::AddArrangementSection {
                        name: "intro".into(),
                        color: hex_project::ElementColor::Cyan,
                        start: BeatTime::ZERO,
                        end: BeatTime::from_quarters(2).unwrap(),
                    })
                    .unwrap()
                else {
                    panic!("section")
                };
                let full = editor.project().clone();
                let mut old = ordinary_mutable_playback(&full, registry);
                old.apply(SessionPlaybackCommand::Play).unwrap();
                old.render(&mut [[0.0; 2]; 64]).unwrap();
                editor
                    .execute(Command::DuplicateArrangementSection(section))
                    .unwrap();
                let mut shortened = ordinary_mutable_playback(editor.project(), registry);
                shortened.try_inherit_state_from(&mut old).unwrap();
                shortened.render(&mut vec![[0.0; 2]; elapsed]).unwrap();
                assert!(editor.undo());
                let clock = (shortened.arrangement_frame(), shortened.performance_frame());
                let mut conflicting = editor.project().snapshot();
                arrangement(&mut conflicting)[0].notes[0].pitch += 1;
                let mut refused = ordinary_mutable_playback(
                    &Project::from_snapshot(conflicting).unwrap(),
                    registry,
                );
                assert_eq!(
                    refused.try_inherit_state_from(&mut shortened),
                    Err(SessionPlaybackError::MutableInstrumentChase)
                );
                let mut restored = ordinary_mutable_playback(editor.project(), registry);
                restored.release_capacity = 0;
                assert_eq!(
                    restored.try_inherit_state_from(&mut shortened),
                    Err(SessionPlaybackError::ReleaseBankFull)
                );
                assert_eq!(
                    (shortened.arrangement_frame(), shortened.performance_frame()),
                    clock
                );
                assert_eq!(restored.performance_frame(), 0);
                assert!(restored.suppressions.is_empty());
                restored.release_capacity = SESSION_RELEASE_CAPACITY;
                restored.try_inherit_state_from(&mut shortened).unwrap();
                let mut output = [[0.0; 2]; 64];
                restored.render(&mut output).unwrap();
                if looped || elapsed == 64 {
                    assert!(
                        output.iter().any(|frame| frame[0] > 0.0),
                        "retain the still-gated old occurrence"
                    );
                } else {
                    let original = full.tracks()[0].midi_clips()[0].id();
                    let ended = |proof: crate::mutable_instrument::MutableOccurrence| {
                        proof.signature.key().clip != original || proof.note_off_sent
                    };
                    assert!(
                        restored
                            .mutable_instruments
                            .iter()
                            .flatten()
                            .all(|runtime| runtime.proofs().all(ended))
                    );
                    assert!(
                        restored
                            .tail_mutable_instruments
                            .iter()
                            .flatten()
                            .all(|tail| tail.runtime.proofs().all(ended)),
                        "an ended prefix gate must not be resurrected; later copied clips may still sound"
                    );
                }
                // A later publication retains the suppression, rather than chasing again.
                let mut successor = ordinary_mutable_playback(editor.project(), registry);
                successor.try_inherit_state_from(&mut restored).unwrap();
                if looped {
                    let remainder = 48_000 - successor.arrangement_frame();
                    successor
                        .render(&mut vec![[0.0; 2]; remainder as usize])
                        .unwrap();
                    successor.render(&mut output).unwrap();
                    assert!(
                        output.iter().any(|frame| frame[0] > 0.0),
                        "the next pass uses the restored authored occurrence"
                    );
                }
            }
        }
    });
}

#[test]
fn live_section_midi_guard_distinguishes_prefixes_from_relocation_and_exposed_onsets() {
    registry_test(|registry, id| {
        for later_note in [false, true] {
            let base = with_test_instrument(&project(1.0, 0, 6 << 32), id, 0);
            let clip = base.tracks()[0].midi_clips()[0].id();
            let mut editor = Editor::new(base, 16).unwrap();
            if later_note {
                editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::from_quarters(3).unwrap(),
                        duration: BeatTime::from_quarters(1).unwrap(),
                        pitch: 64,
                        channel: 0,
                        velocity: 100,
                    })
                    .unwrap();
            }
            let Created::ArrangementSection(intro) = editor
                .execute(Command::AddArrangementSection {
                    name: "intro".into(),
                    color: hex_project::ElementColor::Cyan,
                    start: BeatTime::ZERO,
                    end: BeatTime::from_quarters(2).unwrap(),
                })
                .unwrap()
            else {
                panic!("section")
            };
            let Created::ArrangementSection(chorus) = editor
                .execute(Command::AddArrangementSection {
                    name: "chorus".into(),
                    color: hex_project::ElementColor::Rose,
                    start: BeatTime::from_quarters(4).unwrap(),
                    end: BeatTime::from_quarters(7).unwrap(),
                })
                .unwrap()
            else {
                panic!("section")
            };
            let full = ordinary_mutable_playback(editor.project(), registry);
            let legacy = SessionPlayback::new(revision_with_instrument_registry(
                editor.project(),
                Arc::new(hex_devices::SineTable::new()),
                registry,
            ))
            .unwrap();
            assert!(
                legacy
                    .revision
                    .section_midi_relocation_requires_stop(&full.revision)
            );
            editor
                .execute(Command::RenameArrangementSection {
                    section: intro,
                    name: "renamed".into(),
                })
                .unwrap();
            let metadata = ordinary_mutable_playback(editor.project(), registry);
            assert!(
                !full
                    .revision
                    .section_midi_relocation_requires_stop(&metadata.revision)
            );
            editor
                .execute(Command::DuplicateArrangementSection(intro))
                .unwrap();
            let prefix = ordinary_mutable_playback(editor.project(), registry);
            assert!(
                !metadata
                    .revision
                    .section_midi_relocation_requires_stop(&prefix.revision)
            );
            assert_eq!(
                prefix
                    .revision
                    .section_midi_relocation_requires_stop(&metadata.revision),
                later_note
            );
            assert!(editor.undo());
            editor
                .execute(Command::ReorderSectionAfter {
                    section: intro,
                    after: chorus,
                })
                .unwrap();
            let moved = ordinary_mutable_playback(editor.project(), registry);
            assert!(
                metadata
                    .revision
                    .section_midi_relocation_requires_stop(&moved.revision)
            );
            assert!(
                moved
                    .revision
                    .section_midi_relocation_requires_stop(&metadata.revision)
            );
        }
    });
}
