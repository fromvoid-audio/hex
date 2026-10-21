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
        start: BeatTime::ZERO,
        duration: BeatTime::from_raw(1 << 32),
        pitch,
        channel: 0,
        velocity: 100,
    }
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
