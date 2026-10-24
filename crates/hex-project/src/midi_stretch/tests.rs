use super::*;
use crate::{
    ClipId, Command, Editor, MidiClipGain, MidiClipVolumePoint, MidiOneShotBoundary, TrackId,
};

fn beat(quarters: u64) -> BeatTime {
    BeatTime::from_quarters(quarters).unwrap()
}

fn fixture(looping: bool) -> (Project, ClipId, TrackId) {
    let mut project = Project::new(48_000).unwrap();
    let Created::Track(track) = project
        .apply(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = project
        .apply(Command::PlaceMidiClip {
            track,
            start: beat(4),
            length: beat(4),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    project
        .apply(Command::SetMidiClipSource {
            clip,
            source_length: beat(8),
            source_start: beat(1),
            loop_start: beat(2),
            loop_end: beat(6),
            looping,
            length: beat(4),
        })
        .unwrap();
    for (start, pitch) in [(2, 60), (6, 64)] {
        project
            .apply(Command::AddMidiNote {
                clip,
                start: beat(start),
                duration: beat(1),
                pitch,
                channel: 2,
                velocity: 73,
            })
            .unwrap();
    }
    let points = vec![
        MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::new(0.25).unwrap()),
        MidiClipVolumePoint {
            beat: beat(2),
            incoming: MidiClipGain::new(0.5).unwrap(),
            value: MidiClipGain::new(1.5).unwrap(),
        },
        MidiClipVolumePoint::linear(beat(8), MidiClipGain::new(0.75).unwrap()),
    ];
    project
        .apply(Command::ReplaceMidiClipVolume {
            clip,
            volume: MidiClipVolume::new(points, beat(8)).unwrap(),
        })
        .unwrap();
    // Nonrepeating LoopEnd retains the authored note at6 beyond its audible end.
    if !looping {
        let target = project
            .midi_clips_mut(clip)
            .unwrap()
            .iter_mut()
            .find(|c| c.id() == clip)
            .unwrap();
        Arc::make_mut(&mut target.source).one_shot_boundary = MidiOneShotBoundary::LoopEnd;
    }
    let Created::Track(other) = project
        .apply(Command::AddTrack {
            name: "shared copy".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    (project, clip, other)
}

#[test]
fn both_edges_expand_and_compress_all_retained_source_coordinates() {
    for looping in [false, true] {
        for (edge, destination) in [
            (TrimEdge::Right, 12),
            (TrimEdge::Left, 0),
            (TrimEdge::Right, 6),
            (TrimEdge::Left, 6),
        ] {
            let (project, clip, _) = fixture(looping);
            let before = project.midi_clip(clip).unwrap().clone();
            let preview = before
                .preview_edge_stretch(edge, beat(destination))
                .unwrap();
            let mut editor = Editor::new(project, 8).unwrap();
            editor
                .execute(Command::StretchMidiClipEdge {
                    clip,
                    edge,
                    beat: beat(destination),
                })
                .unwrap();
            let after = editor.project().midi_clip(clip).unwrap();
            assert_eq!(after.start(), preview.start());
            assert_eq!(after.length(), preview.length());
            assert_eq!(
                after.source_start(),
                preview.scale(before.source_start()).unwrap()
            );
            assert_eq!(
                after.source_length(),
                preview.scale(before.source_length()).unwrap()
            );
            assert_eq!(
                after.loop_start(),
                preview.scale(before.loop_start()).unwrap()
            );
            assert_eq!(after.loop_end(), preview.scale(before.loop_end()).unwrap());
            assert_eq!(after.looping(), looping);
            assert_eq!(after.one_shot_boundary(), before.one_shot_boundary());
            assert_eq!(after.notes().len(), before.notes().len());
            for (old, new) in before.notes().iter().zip(after.notes()) {
                assert_eq!(
                    (new.id(), new.pitch(), new.channel(), new.velocity()),
                    (old.id(), old.pitch(), old.channel(), old.velocity())
                );
                assert_eq!(new.start(), preview.scale(old.start()).unwrap());
                assert_eq!(
                    new.start().raw() + new.duration().raw(),
                    preview
                        .scale(BeatTime::from_raw(old.start().raw() + old.duration().raw()))
                        .unwrap()
                        .raw()
                );
            }
            for (old, new) in before.volume().points().iter().zip(after.volume().points()) {
                assert_eq!(new.beat, preview.scale(old.beat).unwrap());
                assert_eq!((new.incoming, new.value), (old.incoming, old.value));
            }
            assert_eq!(after.id(), clip);
            assert_eq!(after.name(), before.name());
            assert_eq!(after.flavor(), before.flavor());
            if edge == TrimEdge::Left {
                assert_eq!(
                    after.start().raw() + after.length().raw(),
                    before.start().raw() + before.length().raw()
                );
            } else {
                assert_eq!(after.start(), before.start());
            }
            assert_eq!(
                Project::from_snapshot(editor.project().snapshot()).unwrap(),
                *editor.project()
            );
        }
    }
}

#[test]
fn stretching_detaches_only_edited_copy_and_one_history_step_round_trips_complete_state() {
    let (mut project, clip, other) = fixture(true);
    let Created::Clip(copy) = project
        .apply(Command::CopyMidiClipTo {
            clip,
            track: other,
            start: BeatTime::ZERO,
        })
        .unwrap()
    else {
        panic!("copy")
    };
    let identity = project.midi_clip(clip).unwrap().source_identity();
    // Canonical copies own fresh note IDs and therefore a distinct source;
    // snapshots/history share each unchanged source through its owning Arc.
    let copy_identity = project.midi_clip(copy).unwrap().source_identity();
    assert_ne!(identity, copy_identity);
    let before = project.clone();
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::StretchMidiClipEdge {
            clip,
            edge: TrimEdge::Left,
            beat: beat(0),
        })
        .unwrap();
    let after = editor.project().clone();
    assert_eq!(after.midi_clip(copy), before.midi_clip(copy));
    assert_eq!(
        after.midi_clip(copy).unwrap().source_identity(),
        copy_identity
    );
    assert_ne!(after.midi_clip(clip).unwrap().source_identity(), identity);
    assert!(
        !after
            .midi_clip(clip)
            .unwrap()
            .volume()
            .same_revision(before.midi_clip(clip).unwrap().volume())
    );
    assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert_eq!(
        editor.project().midi_clip(clip).unwrap().source_identity(),
        identity
    );
    assert!(!editor.can_undo(), "one stretch owns one history edge");
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);
    assert!(!editor.can_redo());
}

#[test]
fn endpoint_rounding_preserves_ids_and_derives_duration_from_both_scaled_ends() {
    let (mut project, clip, _) = fixture(false);
    let Created::Note(note) = project
        .apply(Command::AddMidiNote {
            clip,
            start: BeatTime::from_raw(3),
            duration: BeatTime::from_raw(3),
            pitch: 90,
            channel: 4,
            velocity: 101,
        })
        .unwrap()
    else {
        panic!("note")
    };
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::StretchMidiClipEdge {
            clip,
            edge: TrimEdge::Right,
            beat: beat(10),
        })
        .unwrap();
    let note = editor
        .project()
        .midi_clip(clip)
        .unwrap()
        .notes()
        .iter()
        .find(|n| n.id() == note)
        .unwrap();
    assert_eq!(note.start().raw(), 4);
    assert_eq!(
        note.duration().raw(),
        5,
        "floor(6*3/2)-floor(3*3/2), not floor(duration*3/2)"
    );
}

fn assert_refused(editor: &mut Editor, command: Command, expected: &str) {
    let before = editor.project().clone();
    let (undo, redo) = (editor.can_undo(), editor.can_redo());
    let error = editor.execute(command).unwrap_err();
    assert!(
        error.to_string().contains(expected),
        "unexpected refusal: {error}"
    );
    assert_eq!(editor.project(), &before);
    assert_eq!((editor.can_undo(), editor.can_redo()), (undo, redo));
}

#[test]
fn collapsed_unsampled_notes_and_curve_boundaries_refuse_atomically_with_redo_intact() {
    let (project, clip, _) = fixture(true);
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::StretchMidiClipEdge {
            clip,
            edge: TrimEdge::Right,
            beat: beat(12),
        })
        .unwrap();
    assert!(editor.undo());
    let endpoint = BeatTime::from_raw(beat(4).raw() + 1);
    let candidate = editor
        .project()
        .midi_clip(clip)
        .unwrap()
        .preview_edge_stretch(TrimEdge::Right, endpoint)
        .unwrap();
    assert_eq!(
        candidate.length().raw(),
        1,
        "pointer metadata is O(1), full note validation belongs to commit"
    );
    assert_refused(
        &mut editor,
        Command::StretchMidiClipEdge {
            clip,
            edge: TrimEdge::Right,
            beat: endpoint,
        },
        "collapses a note",
    );
    assert!(
        editor.redo(),
        "refused compression must preserve existing redo"
    );

    let (mut project, clip, _) = fixture(false);
    let volume = MidiClipVolume::new(
        vec![
            MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::UNITY),
            MidiClipVolumePoint::linear(BeatTime::from_raw(1), MidiClipGain::new(0.5).unwrap()),
        ],
        beat(8),
    )
    .unwrap();
    project
        .apply(Command::ReplaceMidiClipVolume { clip, volume })
        .unwrap();
    let mut editor = Editor::new(project, 8).unwrap();
    assert_refused(
        &mut editor,
        Command::StretchMidiClipEdge {
            clip,
            edge: TrimEdge::Right,
            beat: beat(6),
        },
        "collapses volume boundaries",
    );
}

#[test]
fn crossing_edges_source_overflow_and_collapsed_loops_refuse_without_history_or_id_changes() {
    let (mut project, clip, _) = fixture(true);
    let original = project.midi_clip(clip).unwrap().clone();
    assert!(
        original
            .preview_edge_stretch(TrimEdge::Right, beat(4))
            .is_err()
    );
    assert!(
        original
            .preview_edge_stretch(TrimEdge::Right, beat(3))
            .is_err()
    );
    assert!(
        original
            .preview_edge_stretch(TrimEdge::Left, beat(8))
            .is_err()
    );
    assert!(
        original
            .preview_edge_stretch(TrimEdge::Left, beat(9))
            .is_err()
    );
    let candidate = original
        .preview_edge_stretch(TrimEdge::Right, beat(12))
        .unwrap();
    assert!(candidate.scale(BeatTime::from_raw(u64::MAX)).is_err());
    project
        .apply(Command::SetMidiClipSource {
            clip,
            source_length: BeatTime::from_raw(u64::MAX),
            source_start: beat(1),
            loop_start: beat(2),
            loop_end: beat(6),
            looping: true,
            length: beat(4),
        })
        .unwrap();
    let mut editor = Editor::new(project, 8).unwrap();
    assert_refused(
        &mut editor,
        Command::StretchMidiClipEdge {
            clip,
            edge: TrimEdge::Right,
            beat: beat(12),
        },
        "source overflow",
    );
    let (mut project, clip, _) = fixture(true);
    project
        .apply(Command::SetMidiClipSource {
            clip,
            source_length: beat(8),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::from_raw(1),
            loop_end: BeatTime::from_raw(2),
            looping: true,
            length: beat(4),
        })
        .unwrap();
    let mut editor = Editor::new(project, 8).unwrap();
    assert_refused(
        &mut editor,
        Command::StretchMidiClipEdge {
            clip,
            edge: TrimEdge::Right,
            beat: beat(5),
        },
        "loop must be inside",
    );
}

#[test]
fn unchanged_edge_keeps_source_and_curve_identity_without_a_history_entry() {
    let (project, clip, _) = fixture(false);
    let before = project.midi_clip(clip).unwrap().clone();
    let mut editor = Editor::new(project, 8).unwrap();
    for (edge, endpoint) in [(TrimEdge::Left, 4), (TrimEdge::Right, 8)] {
        editor
            .execute(Command::StretchMidiClipEdge {
                clip,
                edge,
                beat: beat(endpoint),
            })
            .unwrap();
        let after = editor.project().midi_clip(clip).unwrap();
        assert_eq!(before.source_identity(), after.source_identity());
        assert!(before.volume().same_revision(after.volume()));
        assert!(!editor.can_undo());
    }
}
