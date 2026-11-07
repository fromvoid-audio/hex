use super::*;
use crate::{Editor, MidiNoteInsert};
fn fixture() -> (Editor, TrackId, SceneId, ClipId) {
    let mut p = Project::new(48000).unwrap();
    let Created::Track(track) = p
        .apply(Command::AddTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Scene(scene) = p.apply(Command::AddScene { name: "one".into() }).unwrap() else {
        panic!("scene")
    };
    let Created::Clip(clip) = p
        .apply(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_raw(4 << 32),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    (Editor::new(p, 16).unwrap(), track, scene, clip)
}
fn note(pitch: u8) -> MidiNoteInsert {
    MidiNoteInsert {
        active: true,
        start: BeatTime::ZERO,
        duration: BeatTime::from_raw(1 << 32),
        pitch,
        channel: 0,
        velocity: 100,
    }
}

#[test]
fn retained_source_path_change_is_not_a_midi_only_history_edge() {
    let (editor, _, scene, _) = fixture();
    let original = editor.project();
    let mut changed = original.clone();
    changed.tracks[0].retained_source_path = true;
    let kind = Kind::AppendScene { scene };
    assert!(same_shell(original, original, None, kind));
    assert!(!same_shell(original, &changed, None, kind));
}

#[test]
fn clip_gain_is_independent_of_curve_and_has_valid_history() {
    let (mut editor, _, _, clip) = fixture();
    let length = BeatTime::from_raw(4 << 32);
    let curve = crate::MidiClipVolume::new(
        vec![crate::MidiClipVolumePoint::linear(
            BeatTime::ZERO,
            crate::MidiClipGain::new(0.25).unwrap(),
        )],
        length,
    )
    .unwrap();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: curve.clone(),
        })
        .unwrap();
    let gain = crate::ClipGain::new(-6_000).unwrap();
    cycle(&mut editor, Command::SetMidiClipGain { clip, gain }, true);
    let volume = editor.project().midi_clip_volume(clip).unwrap();
    assert_eq!(volume.points(), curve.points());
    assert_eq!(volume.clip_gain(), gain);
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: crate::MidiClipVolume::default(),
        })
        .unwrap();
    let volume = editor.project().midi_clip_volume(clip).unwrap();
    assert!(volume.points().is_empty());
    assert_eq!(volume.clip_gain(), gain);
    assert!(editor.undo());
    assert_eq!(
        editor.project().midi_clip_volume(clip).unwrap().points(),
        curve.points()
    );
    assert_eq!(
        editor.project().midi_clip_volume(clip).unwrap().clip_gain(),
        gain
    );
}
fn cycle(editor: &mut Editor, command: Command, prepared: bool) {
    let old = editor.project().clone();
    if prepared {
        editor
            .execute_midi_prepared_transition::<_, Invalid>(command, |_, _| Ok(()))
            .unwrap();
    } else {
        editor.execute(command).unwrap();
    }
    let effect = editor
        .next_midi_history_effect(false)
        .expect("canonical origin");
    let changed = editor.project().clone();
    assert!(effect.validates_forward(&old, &changed));
    if prepared {
        editor
            .undo_prepared_transition::<_, Invalid>(|source, target| {
                assert!(effect.validates_forward(target, source));
                Ok(())
            })
            .unwrap()
            .unwrap();
    } else {
        assert!(editor.undo());
    }
    let effect = editor
        .next_midi_history_effect(true)
        .expect("origin moves with edge");
    assert!(effect.validates_forward(editor.project(), &changed));
    if prepared {
        editor
            .redo_prepared_transition::<_, Invalid>(|source, target| {
                assert!(effect.validates_forward(source, target));
                Ok(())
            })
            .unwrap()
            .unwrap();
    } else {
        assert!(editor.redo());
    }
    assert_eq!(editor.project(), &changed);
}
#[test]
fn midi_history_plain_and_prepared_edges_follow_one_global_order() {
    for prepared in [false, true] {
        let (mut e, track, scene, clip) = fixture();
        cycle(
            &mut e,
            Command::AddMidiNotes {
                clip,
                notes: vec![note(60), note(64)],
            },
            prepared,
        );
        cycle(
            &mut e,
            Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 72,
                channel: 0,
                velocity: 127,
            },
            prepared,
        );
        let c = e.project().midi_clip(clip).unwrap();
        let source_command = Command::SetMidiClipSource {
            clip,
            source_length: c.source_length(),
            source_start: c.source_start(),
            loop_start: c.loop_start(),
            loop_end: c.loop_end(),
            looping: c.looping(),
            length: BeatTime::from_raw(5 << 32),
        };
        cycle(&mut e, source_command, prepared);
        let first = e.project().midi_source(clip).unwrap().notes()[0].id();
        cycle(
            &mut e,
            Command::UpdateMidiNote {
                note: first,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(1 << 32),
                pitch: 72,
                channel: 0,
                velocity: 99,
            },
            prepared,
        );
        cycle(
            &mut e,
            Command::EditMidiNotes {
                clip,
                remove: vec![first],
                insert: vec![note(67)],
            },
            prepared,
        );
        cycle(
            &mut e,
            Command::CopyArrangementMidiToSession {
                source: clip,
                scene,
            },
            prepared,
        );
        cycle(&mut e, Command::ClearSessionCell { track, scene }, prepared);
        cycle(
            &mut e,
            Command::CreateSessionMidiClip {
                track,
                scene,
                name: "cell".into(),
                source_length: BeatTime::from_raw(4 << 32),
            },
            prepared,
        );
        cycle(&mut e, Command::DuplicateMidiClip(clip), prepared);
        cycle(&mut e, Command::RemoveMidiClip(clip), prepared);
        cycle(&mut e, Command::AddScene { name: "two".into() }, prepared);
        // An unrelated adjacent edge is never skipped in favor of old MIDI.
        e.execute(Command::MasterGain { gain: 0.5 }).unwrap();
        assert!(e.next_midi_history_effect(false).is_none());
        assert!(e.undo());
        assert!(e.next_midi_history_effect(false).is_some());
        assert!(e.redo());
        assert!(e.next_midi_history_effect(false).is_none());
    }
}
#[test]
fn midi_history_refusal_preserves_stacks_origins_and_redo() {
    let (mut e, _, _, clip) = fixture();
    e.execute(Command::AddMidiNotes {
        clip,
        notes: vec![note(60)],
    })
    .unwrap();
    let effect = e.next_midi_history_effect(false).unwrap();
    let current = e.project().clone();
    assert!(
        e.undo_prepared_transition::<(), Invalid>(|_, _| Err(Invalid("native refused")))
            .is_err()
    );
    assert_eq!(e.project(), &current);
    assert!(
        e.inspect_history(false, |source, target| effect
            .validates_forward(target, source))
            .unwrap()
    );
    assert!(e.undo());
    let before = e.project().clone();
    assert!(
        e.execute_midi_prepared_transition(
            Command::AddMidiNotes {
                clip,
                notes: vec![note(72)]
            },
            |_, _| Err::<(), _>(Invalid("sealed refusal"))
        )
        .is_err()
    );
    assert_eq!(e.project(), &before);
    assert!(e.next_midi_history_effect(true).is_some());
    assert!(
        e.execute_midi_prepared_transition::<(), Invalid>(
            Command::MasterGain { gain: 0.25 },
            |_, _| panic!("unsupported before prepare")
        )
        .is_err()
    );
    assert!(e.can_redo());
    e.redo();
    // Even a singleton batch has no single-command origin.
    e.execute_many_prepared(
        vec![Command::AddMidiNotes {
            clip,
            notes: vec![note(67)],
        }],
        |_, _| Ok::<_, Invalid>(()),
    )
    .unwrap();
    assert!(e.next_midi_history_effect(false).is_none());
}
#[test]
fn midi_history_scope_rejects_other_state_and_overlapping_paint_before_prepare() {
    let (mut e, track, _, clip) = fixture();
    let old = e.project().clone();
    e.execute(Command::AddMidiNotes {
        clip,
        notes: vec![note(60)],
    })
    .unwrap();
    let effect = e.next_midi_history_effect(false).unwrap();
    let mut target = e.project().clone();
    target.tracks[0].name = "different".into();
    assert!(!effect.validates_forward(&old, &target));
    target = e.project().clone();
    target.session.scenes[0].name = "different".into();
    assert!(!effect.validates_forward(&old, &target));
    target = e.project().clone();
    target.tracks[0].content.midi_clips[0].start = BeatTime::from_raw(1);
    assert!(!effect.validates_forward(&old, &target));
    let before = e.project().clone();
    assert!(
        e.execute_midi_prepared_transition::<(), Invalid>(
            Command::PlaceMidiClipWithOverwrite {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_raw(1 << 32)
            },
            |_, _| panic!("overlap before prepare")
        )
        .is_err()
    );
    assert_eq!(e.project(), &before);
    cycle(
        &mut e,
        Command::PlaceMidiClipWithOverwrite {
            track,
            start: BeatTime::from_raw(8 << 32),
            length: BeatTime::from_raw(1 << 32),
        },
        true,
    );
}
#[test]
fn midi_noop_preflight_is_borrowed_validated_and_preserves_redo() {
    let (mut e, _, _, clip) = fixture();
    e.execute(Command::AddMidiNotes {
        clip,
        notes: vec![note(60)],
    })
    .unwrap();
    let n = e.project().midi_source(clip).unwrap().notes()[0].clone();
    e.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(e.undo());
    assert!(e.can_redo());
    let mut command = Command::UpdateMidiNotes {
        clip,
        notes: vec![crate::MidiNoteUpdate {
            note: n.id,
            start: n.start,
            duration: n.duration,
            pitch: n.pitch,
            channel: n.channel,
            velocity: n.velocity,
        }],
    };
    assert!(!command.prepare_sampler_midi_edit(e.project()).unwrap());
    let changed = e.project().clone();
    e.execute_midi_prepared_transition::<_, Invalid>(command, |_, _| Ok(()))
        .unwrap();
    assert_eq!(e.project(), &changed);
    assert!(e.can_redo());
    let before = e.project().clone();
    assert!(
        Command::UpdateMidiNotes {
            clip,
            notes: vec![]
        }
        .prepare_sampler_midi_edit(&before)
        .is_err()
    );
    assert_eq!(e.project(), &before);
    assert!(e.can_redo());
}

#[test]
fn source_extent_note_edit_has_one_closed_history_edge_for_arrange_and_console() {
    for console in [false, true] {
        let (mut editor, track, scene, mut clip) = fixture();
        if console {
            let Created::SessionClip(id) = editor
                .execute(Command::CreateSessionMidiClip {
                    track,
                    scene,
                    name: "extent".into(),
                    source_length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("console");
            };
            clip = id;
        }
        let before = editor.project().clone();
        let mut inserted = note(60);
        inserted.start = BeatTime::from_quarters(5).unwrap();
        let command = || Command::EditMidiNotesWithSourceExtent {
            clip,
            remove: Vec::new(),
            insert: vec![inserted],
        };
        cycle(&mut editor, command(), true);
        let changed = editor.project().clone();
        let source = changed.midi_source(clip).unwrap();
        assert_eq!(source.source_length(), BeatTime::from_quarters(6).unwrap());
        assert_eq!(
            source.loop_end(),
            before.midi_source(clip).unwrap().loop_end()
        );
        assert_eq!(Project::from_snapshot(changed.snapshot()).unwrap(), changed);
        let effect = editor.next_midi_history_effect(false).unwrap();
        let mut corrupted = changed.clone();
        corrupted.midi_source_mut(clip).unwrap().source_length =
            BeatTime::from_quarters(7).unwrap();
        assert!(
            !effect.validates_forward(&before, &corrupted),
            "extent cannot exceed inserted end"
        );
        corrupted = changed.clone();
        corrupted.midi_source_mut(clip).unwrap().looping = !source.looping();
        assert!(
            !effect.validates_forward(&before, &corrupted),
            "loop edit cannot hide inside note proof"
        );
        assert!(editor.undo());
        assert_eq!(editor.project().midi_source(clip), before.midi_source(clip));
        assert!(editor.redo());
        assert_eq!(
            editor.project().midi_source(clip),
            changed.midi_source(clip)
        );
    }
}

#[test]
fn source_extent_note_edit_refusals_preserve_source_ids_history_and_strict_commands() {
    let (mut editor, _, _, clip) = fixture();
    let original = editor.project().clone();
    let mut inserted = note(60);
    inserted.start = BeatTime::from_quarters(5).unwrap();
    assert!(
        editor
            .execute(Command::AddMidiNotes {
                clip,
                notes: vec![inserted]
            })
            .is_err()
    );
    assert!(
        editor
            .execute(Command::EditMidiNotesWithSourceExtent {
                clip,
                remove: vec![NoteId(u64::MAX)],
                insert: vec![inserted],
            })
            .is_err()
    );
    assert_eq!(editor.project(), &original);
    for (start, duration, pitch) in [(5 << 32, 0, 60), (u64::MAX, 1, 60), (5 << 32, 1 << 32, 128)] {
        let mut bad = inserted;
        bad.start = BeatTime::from_raw(start);
        bad.duration = BeatTime::from_raw(duration);
        bad.pitch = pitch;
        assert!(
            editor
                .execute(Command::EditMidiNotesWithSourceExtent {
                    clip,
                    remove: Vec::new(),
                    insert: vec![bad]
                })
                .is_err()
        );
        assert_eq!(editor.project(), &original);
    }
    assert!(
        editor
            .execute_validated::<Invalid>(
                Command::EditMidiNotesWithSourceExtent {
                    clip,
                    remove: Vec::new(),
                    insert: vec![inserted]
                },
                |_| Err(Invalid("prepare refused"))
            )
            .is_err()
    );
    assert_eq!(editor.project(), &original);
    assert!(editor.next_midi_history_effect(false).is_none());
}

#[test]
fn stable_note_extent_update_has_a_closed_prepared_history_edge() {
    for console in [false, true] {
        let (mut editor, track, scene, mut clip) = fixture();
        if console {
            let Created::SessionClip(id) = editor
                .execute(Command::CreateSessionMidiClip {
                    track,
                    scene,
                    name: "extent update".into(),
                    source_length: BeatTime::from_quarters(4).unwrap(),
                })
                .unwrap()
            else {
                panic!("console")
            };
            clip = id;
        }
        let Created::Note(id) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap()
        else {
            panic!("note")
        };
        let before = editor.project().clone();
        cycle(
            &mut editor,
            Command::UpdateMidiNotesWithSourceExtent {
                clip,
                notes: vec![crate::MidiNoteUpdate {
                    note: id,
                    start: BeatTime::from_quarters(5).unwrap(),
                    duration: BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                }],
            },
            true,
        );
        let changed = editor.project().clone();
        let effect = editor.next_midi_history_effect(false).unwrap();
        let mut corrupted = changed.clone();
        corrupted.midi_source_mut(clip).unwrap().source_length =
            BeatTime::from_quarters(7).unwrap();
        assert!(
            !effect.validates_forward(&before, &corrupted),
            "growth must match requested note ends"
        );
        corrupted = changed.clone();
        corrupted.midi_source_mut(clip).unwrap().looping = true;
        assert!(
            !effect.validates_forward(&before, &corrupted),
            "loop changes cannot hide inside a note update"
        );
    }
}
