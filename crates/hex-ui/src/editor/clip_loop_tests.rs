//! PR445 capability checkpoint: existing MIDI commands, not shipped gesture wiring.
//! The scalar planner below is deliberately private until the common audio/gesture seam is settled.
use super::midi_source_controls::SourceValues;
use hex_project::{
    BeatTime, ClipId, Command, Created, Editor, Invalid, MidiClip, MidiSourcePlayback, Project,
};

const Q: u64 = 1 << 32;
fn beat(value: u64) -> BeatTime {
    BeatTime::from_raw(value)
}

#[derive(Clone, Copy)]
enum Gesture {
    Outer,
    FirstBoundary,
}

fn values(clip: &MidiClip, gesture: Gesture, target: BeatTime) -> Result<SourceValues, Invalid> {
    let mut next =
        SourceValues::from_source(clip.source(), clip.start().raw(), clip.length().raw());
    let offset = target
        .raw()
        .checked_sub(next.arrangement_start)
        .filter(|length| *length > 0)
        .ok_or(Invalid("loop boundary must follow clip start"))?;
    match gesture {
        Gesture::Outer => {
            if !next.looping {
                // Capture CURRENT visible content, including its nonzero source offset.
                next.loop_start = next.source_start;
                next.loop_end = next
                    .source_start
                    .checked_add(next.arrangement_length)
                    .ok_or(Invalid("first loop source end overflow"))?;
                // One-shot MIDI can contain trailing silence; retain that silence in the cycle.
                next.source_length = next.source_length.max(next.loop_end);
                next.looping = true;
            }
            next.arrangement_length = offset;
        }
        Gesture::FirstBoundary => {
            if !next.looping {
                return Err(Invalid("first repeat needs an existing loop"));
            }
            next.loop_end = next
                .source_start
                .checked_add(offset)
                .ok_or(Invalid("loop source end overflow"))?;
            next.source_length = next.source_length.max(next.loop_end);
        }
    }
    next.arrangement_start
        .checked_add(next.arrangement_length)
        .ok_or(Invalid("loop arrangement end overflow"))?;
    MidiSourcePlayback::new(
        beat(next.source_length),
        beat(next.source_start),
        Some((beat(next.loop_start), beat(next.loop_end))),
        beat(next.arrangement_length),
    )?;
    Ok(next)
}

fn fixture(source_start: u64, length: u64, looping: bool) -> (Editor, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(10 * Q),
            length: beat(8 * Q),
        })
        .unwrap()
    else {
        panic!("clip");
    };
    for position in [Q / 2, 2 * Q + Q / 4, 4 * Q + Q / 2, 6 * Q] {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: beat(position),
                duration: beat(Q / 4),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(8 * Q),
            source_start: beat(source_start),
            loop_start: beat(2 * Q),
            loop_end: beat(4 * Q),
            looping,
            length: beat(length),
        })
        .unwrap();
    (editor, clip)
}
fn overwrite_command(clip: ClipId, next: SourceValues) -> Command {
    Command::SetMidiClipSource {
        clip,
        source_length: beat(next.source_length),
        source_start: beat(next.source_start),
        loop_start: beat(next.loop_start),
        loop_end: beat(next.loop_end),
        looping: next.looping,
        length: beat(next.arrangement_length),
    }
}
fn apply(editor: &mut Editor, clip: ClipId, next: SourceValues) {
    let command = overwrite_command(clip, next);
    editor.execute(command).unwrap();
}
fn occurrences(clip: &MidiClip, note_start: u64) -> Vec<u64> {
    clip.source_playback()
        .unwrap()
        .note_occurrences(beat(note_start), beat(Q / 4))
        .unwrap()
        .iter()
        .map(|note| note.timeline_start().raw())
        .collect()
}

#[test]
fn midi_loop_first_activation_captures_trimmed_source_and_real_repeats_without_note_loss() {
    let (mut editor, clip) = fixture(2 * Q, 2 * Q, false);
    let before = editor.project().clone();
    let source = before.midi_clip(clip).unwrap();
    let source_key = source.source_identity();
    let next = values(source, Gesture::Outer, beat(18 * Q)).unwrap();
    assert_eq!(
        source.source_identity(),
        source_key,
        "pointer planning does not detach authored notes"
    );
    assert_eq!(
        (
            next.source_start,
            next.loop_start,
            next.loop_end,
            next.arrangement_length
        ),
        (2 * Q, 2 * Q, 4 * Q, 8 * Q)
    );
    apply(&mut editor, clip, next);
    let after = editor.project().clone();
    let looping = after.midi_clip(clip).unwrap();
    assert_eq!(looping.notes(), source.notes());
    assert_eq!(
        occurrences(looping, 2 * Q + Q / 4),
        vec![Q / 4, 2 * Q + Q / 4, 4 * Q + Q / 4, 6 * Q + Q / 4]
    );
    assert!(
        occurrences(looping, 4 * Q + Q / 2).is_empty(),
        "hidden source notes remain authored but outside cycle"
    );
    // UI deliberately has no storage dependency: validate its complete model snapshot.
    // Byte save/reopen remains a Session/Storage integration check after gesture wiring.
    assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);
}

#[test]
fn midi_loop_outer_resize_retains_authored_intro_phase_and_loop_region() {
    let (mut editor, clip) = fixture(Q, 8 * Q, true);
    let original = editor.project().midi_clip(clip).unwrap().clone();
    let next = values(&original, Gesture::Outer, beat(22 * Q)).unwrap();
    assert_eq!(
        (
            next.source_start,
            next.loop_start,
            next.loop_end,
            next.source_length
        ),
        (Q, 2 * Q, 4 * Q, 8 * Q)
    );
    apply(&mut editor, clip, next);
    let changed = editor.project().midi_clip(clip).unwrap();
    assert_eq!(changed.length(), beat(12 * Q));
    assert_eq!(
        occurrences(changed, 2 * Q + Q / 4),
        vec![
            Q + Q / 4,
            3 * Q + Q / 4,
            5 * Q + Q / 4,
            7 * Q + Q / 4,
            9 * Q + Q / 4,
            11 * Q + Q / 4
        ]
    );
    assert_eq!(changed.notes(), original.notes());
}

#[test]
fn midi_first_boundary_moves_both_directions_keeping_outer_extent_and_note_ids() {
    for (marker, period) in [(11 * Q, Q), (13 * Q, 3 * Q)] {
        let (mut editor, clip) = fixture(2 * Q, 8 * Q, true);
        let before = editor.project().clone();
        let original = before.midi_clip(clip).unwrap();
        let next = values(original, Gesture::FirstBoundary, beat(marker)).unwrap();
        apply(&mut editor, clip, next);
        let changed = editor.project().midi_clip(clip).unwrap();
        assert_eq!(changed.length(), original.length());
        assert_eq!(changed.start(), original.start());
        assert_eq!(
            changed.loop_end().raw() - changed.loop_start().raw(),
            period
        );
        assert_eq!(changed.notes(), original.notes());
        let starts = occurrences(changed, 2 * Q + Q / 4);
        let expected: Vec<_> = (0..8)
            .map(|i| Q / 4 + i * period)
            .take_while(|start| *start < 8 * Q)
            .collect();
        assert_eq!(starts, expected);
        assert!(editor.undo());
        assert_eq!(editor.project(), &before);
    }
}

#[test]
fn midi_loop_first_cycle_retains_one_shot_trailing_silence() {
    let (mut editor, clip) = fixture(6 * Q, 4 * Q, false);
    let original = editor.project().midi_clip(clip).unwrap().clone();
    let next = values(&original, Gesture::Outer, beat(22 * Q)).unwrap();
    assert_eq!(
        (next.loop_start, next.loop_end, next.source_length),
        (6 * Q, 10 * Q, 10 * Q)
    );
    apply(&mut editor, clip, next);
    let changed = editor.project().midi_clip(clip).unwrap();
    assert_eq!(changed.notes(), original.notes());
    assert_eq!(occurrences(changed, 6 * Q), vec![0, 4 * Q, 8 * Q]);
}

#[test]
fn midi_loop_invalid_and_prepare_refusal_keep_project_ids_history_and_redo() {
    let (mut editor, clip) = fixture(2 * Q, 2 * Q, false);
    editor
        .execute(Command::RenameClip {
            clip,
            name: "redo branch".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let original = before.midi_clip(clip).unwrap();
    for target in [0, 9 * Q, 10 * Q] {
        assert!(values(original, Gesture::Outer, beat(target)).is_err());
    }
    assert!(values(original, Gesture::FirstBoundary, beat(11 * Q)).is_err());
    let next = values(original, Gesture::Outer, beat(18 * Q)).unwrap();
    let mut prepares = 0;
    let command = overwrite_command(clip, next);
    let result: Result<_, Invalid> = editor.execute_prepared(command, |candidate| {
        prepares += 1;
        assert_eq!(candidate.midi_clip(clip).unwrap().length(), beat(8 * Q));
        Err::<(), _>(Invalid("controlled preparation refusal"))
    });
    assert!(result.is_err());
    assert_eq!(prepares, 1);
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
    let mut invalid = next;
    invalid.arrangement_length = 0;
    let result: Result<_, Invalid> =
        editor.execute_prepared(overwrite_command(clip, invalid), |_| {
            prepares += 1;
            Ok(())
        });
    assert!(result.is_err());
    assert_eq!(prepares, 1);
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert_eq!(editor.project().clip_name(clip), Some("redo branch"));
}

#[test]
fn midi_loop_noop_preserves_redo_and_boundary_overflow_refuses() {
    let (mut editor, clip) = fixture(2 * Q, 8 * Q, true);
    editor
        .execute(Command::RenameClip {
            clip,
            name: "redo".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let original = before.midi_clip(clip).unwrap();
    apply(
        &mut editor,
        clip,
        values(original, Gesture::Outer, beat(18 * Q)).unwrap(),
    );
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(16 * Q),
            source_start: beat(12 * Q),
            loop_start: beat(12 * Q),
            loop_end: beat(14 * Q),
            looping: true,
            length: beat(8 * Q),
        })
        .unwrap();
    let high_source = editor.project().midi_clip(clip).unwrap();
    assert!(values(high_source, Gesture::FirstBoundary, beat(u64::MAX)).is_err());
}

fn with_audio_neighbor(
    editor: &mut Editor,
    track: hex_project::TrackId,
    start: u64,
    end: u64,
) -> ClipId {
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: std::path::PathBuf::from("model-only-loop-neighbor.wav"),
            sample_rate: 48_000,
            frames: 1_000_000,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    let first = editor
        .project()
        .tempo_map()
        .beat_to_frame(beat(start))
        .unwrap();
    let last = editor
        .project()
        .tempo_map()
        .beat_to_frame(beat(end))
        .unwrap();
    let Created::Clip(id) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: first,
            duration: last - first,
            source_offset: 100,
            source_length: last - first,
            looping: false,
        })
        .unwrap()
    else {
        panic!("audio neighbor");
    };
    id
}

#[test]
fn midi_single_loop_growth_carves_audio_and_midi_neighbors_at_final_capacity_in_one_undo() {
    for at_capacity in [false, true] {
        let (mut editor, clip) = fixture(2 * Q, 2 * Q, false);
        let track = editor.project().tracks()[0].id();
        let Created::Clip(victim) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(12 * Q),
                length: beat(Q),
            })
            .unwrap()
        else {
            panic!("midi neighbor");
        };
        editor
            .execute(Command::AddMidiNote {
                clip: victim,
                start: beat(Q / 4),
                duration: beat(Q / 4),
                pitch: 72,
                channel: 0,
                velocity: 80,
            })
            .unwrap();
        let audio = with_audio_neighbor(&mut editor, track, 13 * Q, 17 * Q);
        let mut project = editor.project().clone();
        if at_capacity {
            let mut snapshot = project.snapshot();
            let mut filler = match &snapshot.tracks[0].content {
                hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } => {
                    midi_clips[0].clone()
                }
                _ => panic!("hybrid"),
            };
            filler.notes.clear();
            filler.length_raw = Q;
            filler.source_length_raw = Q;
            filler.source_start_raw = 0;
            filler.loop_start_raw = 0;
            filler.loop_end_raw = Q;
            filler.looping = false;
            let mut fillers = Vec::new();
            for i in 0..hex_project::MAX_CLIPS - 3 {
                let mut item = filler.clone();
                item.id = snapshot.next_id;
                snapshot.next_id += 1;
                item.start_raw = (100 + i as u64 * 2) * Q;
                fillers.push(item);
            }
            let hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } =
                &mut snapshot.tracks[0].content
            else {
                panic!("hybrid");
            };
            midi_clips.extend(fillers);
            project = Project::from_snapshot(snapshot).unwrap();
        }
        editor = Editor::new(project, 8).unwrap();
        let before = editor.project().clone();
        let incoming = before.midi_clip(clip).unwrap();
        let old_audio = before.clip(audio).unwrap();
        let next = values(incoming, Gesture::Outer, beat(15 * Q)).unwrap();
        let mut prepares = 0;
        editor
            .execute_prepared(overwrite_command(clip, next), |candidate| {
                prepares += 1;
                assert!(candidate.midi_clip(victim).is_none());
                let tail = candidate.clip(audio).unwrap();
                let new_start = candidate.tempo_map().beat_to_frame(beat(15 * Q)).unwrap();
                assert_eq!(tail.start(), new_start);
                assert_eq!(
                    tail.source_offset(),
                    old_audio.source_offset() + new_start - old_audio.start()
                );
                assert_eq!(
                    tail.start() + tail.duration(),
                    old_audio.start() + old_audio.duration()
                );
                assert_eq!(tail.asset(), old_audio.asset());
                assert_eq!(candidate.midi_clip(clip).unwrap().notes(), incoming.notes());
                assert_eq!(
                    Project::from_snapshot(candidate.snapshot()).unwrap(),
                    *candidate
                );
                Ok::<(), Invalid>(())
            })
            .unwrap();
        assert_eq!(prepares, 1);
        let after = editor.project().clone();
        let count = |project: &Project| {
            project
                .tracks()
                .iter()
                .map(|t| t.clips().len() + t.midi_clips().len())
                .sum::<usize>()
        };
        assert_eq!(count(&after), count(&before) - 1);
        if at_capacity {
            assert_eq!(count(&before), hex_project::MAX_CLIPS);
        }
        assert!(editor.undo());
        assert_eq!(editor.project(), &before);
        assert!(!editor.can_undo());
        assert!(editor.redo());
        assert_eq!(editor.project(), &after);
    }
}

#[test]
fn midi_single_loop_growth_late_midi_fragment_refusal_restores_prior_audio_and_redo() {
    let (mut editor, clip) = fixture(2 * Q, 2 * Q, false);
    let track = editor.project().tracks()[0].id();
    let audio = with_audio_neighbor(&mut editor, track, 13 * Q, 14 * Q);
    let Created::Clip(victim) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(14 * Q),
            length: beat(4096 * Q),
        })
        .unwrap()
    else {
        panic!("dense loop");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: victim,
            start: beat(0),
            duration: beat(Q / 64),
            pitch: 72,
            channel: 0,
            velocity: 80,
        })
        .unwrap();
    editor
        .execute(Command::SetMidiClipSource {
            clip: victim,
            source_length: beat(Q),
            source_start: beat(0),
            loop_start: beat(0),
            loop_end: beat(Q / 32),
            looping: true,
            length: beat(4096 * Q),
        })
        .unwrap();
    editor
        .execute(Command::RenameClip {
            clip,
            name: "redo remains".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let next = values(
        before.midi_clip(clip).unwrap(),
        Gesture::Outer,
        beat(15 * Q),
    )
    .unwrap();
    let mut prepares = 0;
    let failure = editor
        .execute_prepared(overwrite_command(clip, next), |_| {
            prepares += 1;
            Ok::<(), Invalid>(())
        })
        .unwrap_err();
    assert_eq!(
        failure,
        Invalid("midi overwrite note expansion exceeds project limit")
    );
    assert_eq!(prepares, 0);
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.project().clip(audio), before.clip(audio));
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert_eq!(editor.project().clip_name(clip), Some("redo remains"));
}
