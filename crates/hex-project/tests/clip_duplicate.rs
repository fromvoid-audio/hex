use hex_project::{
    BeatTime, ClipId, Command, Created, Editor, Invalid, Project, Tempo, TrackContentSnapshot,
};
use std::path::PathBuf;

fn beat(quarters: u64) -> BeatTime {
    BeatTime::from_quarters(quarters).expect("valid beat")
}

fn track(editor: &mut Editor, name: &str) -> hex_project::TrackId {
    match editor
        .execute(Command::AddTrack { name: name.into() })
        .expect("add track")
    {
        Created::Track(id) => id,
        other => panic!("unexpected track result: {other:?}"),
    }
}

fn midi_clip(
    editor: &mut Editor,
    track: hex_project::TrackId,
    start: BeatTime,
    length: BeatTime,
) -> ClipId {
    match editor
        .execute(Command::PlaceMidiClip {
            track,
            start,
            length,
        })
        .expect("place midi clip")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected midi result: {other:?}"),
    }
}

fn audio_clip(
    editor: &mut Editor,
    track: hex_project::TrackId,
    asset: hex_project::AssetId,
    start: u64,
    duration: u64,
    source_offset: u64,
) -> ClipId {
    match editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start,
            duration,
            source_offset,
            source_length: duration,
            looping: true,
        })
        .expect("place audio clip")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected audio result: {other:?}"),
    }
}

fn created_clip_ids(before: &hex_project::Snapshot, after: &hex_project::Snapshot) -> Vec<u64> {
    let old = before
        .tracks
        .iter()
        .flat_map(|track| match &track.content {
            TrackContentSnapshot::Audio(clips) => {
                clips.iter().map(|clip| clip.id).collect::<Vec<_>>()
            }
            TrackContentSnapshot::Instrument(clips) => clips.iter().map(|clip| clip.id).collect(),
            TrackContentSnapshot::Hybrid {
                audio_clips,
                midi_clips,
            } => audio_clips
                .iter()
                .map(|clip| clip.id)
                .chain(midi_clips.iter().map(|clip| clip.id))
                .collect(),
        })
        .collect::<std::collections::BTreeSet<_>>();
    after
        .tracks
        .iter()
        .flat_map(|track| match &track.content {
            TrackContentSnapshot::Audio(clips) => {
                clips.iter().map(|clip| clip.id).collect::<Vec<_>>()
            }
            TrackContentSnapshot::Instrument(clips) => clips.iter().map(|clip| clip.id).collect(),
            TrackContentSnapshot::Hybrid {
                audio_clips,
                midi_clips,
            } => audio_clips
                .iter()
                .map(|clip| clip.id)
                .chain(midi_clips.iter().map(|clip| clip.id))
                .collect(),
        })
        .filter(|id| !old.contains(id))
        .collect()
}

fn audio_snapshot(snapshot: &hex_project::Snapshot, id: u64) -> &hex_project::ClipSnapshot {
    snapshot
        .tracks
        .iter()
        .find_map(|track| match &track.content {
            TrackContentSnapshot::Audio(clips) => clips.iter().find(|clip| clip.id == id),
            TrackContentSnapshot::Hybrid { audio_clips, .. } => {
                audio_clips.iter().find(|clip| clip.id == id)
            }
            TrackContentSnapshot::Instrument(_) => None,
        })
        .expect("audio clip snapshot")
}

fn midi_snapshot(snapshot: &hex_project::Snapshot, id: u64) -> &hex_project::MidiClipSnapshot {
    snapshot
        .tracks
        .iter()
        .find_map(|track| match &track.content {
            TrackContentSnapshot::Instrument(clips) => clips.iter().find(|clip| clip.id == id),
            TrackContentSnapshot::Hybrid { midi_clips, .. } => {
                midi_clips.iter().find(|clip| clip.id == id)
            }
            TrackContentSnapshot::Audio(_) => None,
        })
        .expect("midi clip snapshot")
}

#[test]
fn midi_group_duplicate_uses_exact_beat_extent_across_tempo_changes_and_one_history_step() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let first_track = track(&mut editor, "first");
    let second_track = track(&mut editor, "second");
    let first = midi_clip(&mut editor, first_track, beat(1), beat(2));
    let second = midi_clip(&mut editor, second_track, beat(6), beat(2));
    editor
        .execute(Command::AddMidiNote {
            clip: first,
            start: beat(1),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 91,
        })
        .expect("first note");
    editor
        .execute(Command::SetMidiClipSource {
            clip: second,
            source_length: beat(2),
            source_start: beat(0),
            loop_start: beat(0),
            loop_end: beat(2),
            looping: true,
            length: beat(2),
        })
        .expect("loop second clip");
    editor
        .execute(Command::AddMidiNote {
            clip: second,
            start: beat(0),
            duration: beat(1),
            pitch: 67,
            channel: 2,
            velocity: 103,
        })
        .expect("second note");
    editor
        .execute(Command::SetTempoPoint {
            beat: beat(4),
            tempo: Tempo::new(60_000_000).expect("tempo"),
        })
        .expect("tempo point");

    let before = editor.project().snapshot();
    editor
        .execute(Command::DuplicateClips(vec![second, first]))
        .expect("duplicate selected clips");
    let after = editor.project().snapshot();
    let copies = created_clip_ids(&before, &after);
    assert_eq!(copies.len(), 2);

    for (original_id, expected_start) in [(first, beat(8)), (second, beat(13))] {
        let original = midi_snapshot(&before, original_id.value());
        let copy = copies
            .iter()
            .map(|id| midi_snapshot(&after, *id))
            .find(|clip| clip.start_raw == expected_start.raw())
            .expect("copy at exact musical offset");
        assert_eq!(copy.length_raw, original.length_raw);
        assert_eq!(copy.source_length_raw, original.source_length_raw);
        assert_eq!(copy.source_start_raw, original.source_start_raw);
        assert_eq!(copy.loop_start_raw, original.loop_start_raw);
        assert_eq!(copy.loop_end_raw, original.loop_end_raw);
        assert_eq!(copy.looping, original.looping);
        assert_eq!(copy.volume, original.volume);
        assert_eq!(copy.notes.len(), original.notes.len());
        for (copied_note, original_note) in copy.notes.iter().zip(&original.notes) {
            assert_eq!(copied_note.start_raw, original_note.start_raw);
            assert_eq!(copied_note.duration_raw, original_note.duration_raw);
            assert_eq!(copied_note.pitch, original_note.pitch);
            assert_eq!(copied_note.channel, original_note.channel);
            assert_eq!(copied_note.velocity, original_note.velocity);
            assert_ne!(copied_note.id, original_note.id);
        }
    }

    assert!(editor.undo(), "the entire group is one undo step");
    let restored = editor.project().snapshot();
    let mut before_content = before.clone();
    before_content.next_id = restored.next_id;
    assert_eq!(
        restored, before_content,
        "undo restores all content and keeps IDs monotonic"
    );
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn one_raw_tick_midi_extent_duplicates_without_frame_round_trip() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let track = track(&mut editor, "midi");
    let tiny = BeatTime::from_raw(1);
    let original = midi_clip(&mut editor, track, BeatTime::ZERO, tiny);
    let before = editor.project().snapshot();

    editor
        .execute(Command::DuplicateClips(vec![original]))
        .expect("positive sub-frame selection has a duplicate");

    let after = editor.project().snapshot();
    let copies = created_clip_ids(&before, &after);
    assert_eq!(copies.len(), 1);
    let copy = midi_snapshot(&after, copies[0]);
    assert_eq!(copy.start_raw, 1, "the one-tick span is retained exactly");
    assert_eq!(copy.length_raw, 1);
    assert_eq!(copy.source_length_raw, 1);
    assert_ne!(copy.id, original.value());
}

#[test]
fn mixed_group_duplicate_keeps_gaps_and_source_metadata_while_overwriting_only_copy_footprint() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let audio_track = track(&mut editor, "audio");
    let midi_track = track(&mut editor, "midi");
    let asset = match editor
        .execute(Command::AddAsset {
            path: PathBuf::from("original.wav"),
            sample_rate: 48_000,
            frames: 400_000,
        })
        .expect("asset")
    {
        Created::Asset(id) => id,
        other => panic!("unexpected asset result: {other:?}"),
    };
    let audio_a = audio_clip(&mut editor, audio_track, asset, 0, 1_000, 31_000);
    let audio_b = audio_clip(&mut editor, audio_track, asset, 2_000, 1_000, 35_000);
    editor
        .execute(Command::SetClipFades {
            clip: audio_a,
            fade_in: 175,
            fade_out: 225,
        })
        .expect("set non-default fades");
    let midi = midi_clip(&mut editor, midi_track, beat(4), beat(1));
    editor
        .execute(Command::SetMidiClipSource {
            clip: midi,
            source_length: beat(2),
            source_start: beat(0),
            loop_start: beat(0),
            loop_end: beat(2),
            looping: true,
            length: beat(1),
        })
        .expect("loop midi source");
    editor
        .execute(Command::AddMidiNote {
            clip: midi,
            start: beat(1),
            duration: beat(1),
            pitch: 73,
            channel: 1,
            velocity: 88,
        })
        .expect("author midi event");

    // At 120 BPM the MIDI end is frame 120,000, so the audio anchor's copy
    // starts at frame 120,000. A clip at that exact destination is replaced;
    // a clip just beyond the copied audio remains untouched.
    let victim = audio_clip(&mut editor, audio_track, asset, 120_000, 1_000, 90_000);
    let gap_fill = audio_clip(&mut editor, audio_track, asset, 121_000, 500, 92_000);
    let gap = audio_clip(&mut editor, audio_track, asset, 123_000, 500, 95_000);
    let before = editor.project().snapshot();
    let original_a = audio_snapshot(&before, audio_a.value()).clone();
    let original_b = audio_snapshot(&before, audio_b.value()).clone();
    let original_midi = midi_snapshot(&before, midi.value()).clone();
    let original_gap_fill = audio_snapshot(&before, gap_fill.value()).clone();

    editor
        .execute(Command::DuplicateClips(vec![audio_a, midi, audio_b]))
        .expect("duplicate mixed selection");
    let after = editor.project().snapshot();
    let copies = created_clip_ids(&before, &after);
    assert_eq!(copies.len(), 3);
    assert!(editor.project().clip(victim).is_none());
    assert_eq!(audio_snapshot(&after, gap_fill.value()), &original_gap_fill);
    assert!(editor.project().clip(gap).is_some(), "gap content survives");

    for original in [&original_a, &original_b] {
        let duplicate = editor
            .project()
            .track(audio_track)
            .unwrap()
            .clips()
            .iter()
            .filter(|clip| copies.contains(&clip.id().value()))
            .map(|clip| audio_snapshot(&after, clip.id().value()))
            .find(|clip| {
                clip.asset == original.asset
                    && clip.source_offset == original.source_offset
                    && clip.duration == original.duration
            })
            .expect("corresponding copied audio");
        let mut expected = original.clone();
        expected.id = duplicate.id;
        expected.start = duplicate.start;
        assert_eq!(duplicate, &expected);
        assert!(duplicate.start >= 120_000);
    }
    let midi_copy = editor
        .project()
        .track(midi_track)
        .unwrap()
        .midi_clips()
        .iter()
        .map(|clip| midi_snapshot(&after, clip.id().value()))
        .find(|clip| clip.id != midi.value())
        .expect("copied midi clip");
    let mut expected_midi = original_midi;
    expected_midi.id = midi_copy.id;
    expected_midi.start_raw = midi_copy.start_raw;
    for (copied_note, old_note) in expected_midi.notes.iter_mut().zip(&midi_copy.notes) {
        copied_note.id = old_note.id;
    }
    assert_eq!(midi_copy, &expected_midi);
    assert!(midi_copy.start_raw > beat(4).raw());

    assert!(editor.undo());
    let restored = editor.project().snapshot();
    let mut before_content = before.clone();
    before_content.next_id = restored.next_id;
    assert_eq!(
        restored, before_content,
        "undo restores all content and keeps IDs monotonic"
    );
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn invalid_selection_and_failed_preparation_preserve_project_and_redo() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let track = track(&mut editor, "midi");
    let clip = midi_clip(&mut editor, track, beat(0), beat(2));
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(0),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("note");
    let stale = midi_clip(&mut editor, track, beat(4), beat(1));
    editor
        .execute(Command::RemoveMidiClip(stale))
        .expect("remove stale id");
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(90_000_000).expect("tempo"),
        })
        .expect("history edit");
    assert!(editor.undo());
    let before = editor.project().clone();
    assert!(editor.can_redo());

    assert!(
        editor
            .execute(Command::DuplicateClips(vec![clip, stale]))
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
    assert!(
        editor
            .execute(Command::DuplicateClips(vec![clip, clip]))
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());

    let result: Result<_, Invalid> = editor
        .execute_prepared(Command::DuplicateClips(vec![clip]), |_| {
            Err::<(), _>(Invalid("injected preparation refusal"))
        });
    assert!(result.is_err());
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
}
