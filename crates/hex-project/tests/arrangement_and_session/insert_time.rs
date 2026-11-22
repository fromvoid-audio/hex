use hex_project::{
    BeatTime, Command, Created, Editor, Invalid, MAX_CLIPS, MidiClipSnapshot, Project, Tempo,
    TimeRange, TrackContentSnapshot,
};
use std::sync::Arc;

fn track(editor: &mut Editor, name: &str) -> hex_project::TrackId {
    match editor
        .execute(Command::AddTrack { name: name.into() })
        .expect("track")
    {
        Created::Track(id) => id,
        other => panic!("{other:?}"),
    }
}

fn audio_asset(editor: &mut Editor, path: &str, frames: u64) -> hex_project::AssetId {
    match editor
        .execute(Command::AddAsset {
            path: path.into(),
            sample_rate: 48_000,
            frames,
        })
        .expect("asset")
    {
        Created::Asset(id) => id,
        other => panic!("{other:?}"),
    }
}

fn quarter(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).expect("beat")
}

#[test]
fn insert_opens_full_silent_rectangle_and_only_shifts_selected_rows() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let selected = track(&mut editor, "selected");
    let other = track(&mut editor, "other");
    let source = audio_asset(&mut editor, "source.wav", 240_000);
    let other_source = audio_asset(&mut editor, "other.wav", 240_000);
    editor
        .execute(Command::PlaceClip {
            track: selected,
            asset: source,
            start: 0,
            duration: 96_000,
            source_offset: 100,
            source_length: 120_000,
            looping: true,
        })
        .expect("selected clip");
    editor
        .execute(Command::PlaceClip {
            track: selected,
            asset: source,
            start: 120_000,
            duration: 24_000,
            source_offset: 0,
            source_length: 24_000,
            looping: false,
        })
        .expect("later selected clip");
    editor
        .execute(Command::PlaceClip {
            track: other,
            asset: other_source,
            start: 120_000,
            duration: 24_000,
            source_offset: 0,
            source_length: 24_000,
            looping: false,
        })
        .expect("unselected clip");
    let other_before = editor.project().track(other).expect("other").clips()[0].clone();
    let clipboard = Arc::new(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![selected], 24_000, 120_000).expect("range"))
            .expect("copy"),
    );
    let before = editor.project().snapshot();

    editor
        .execute(Command::InsertTimeRange {
            clipboard,
            track: selected,
            frame: 48_000,
        })
        .expect("insert");

    let clips = editor.project().track(selected).expect("selected").clips();
    assert!(clips.iter().any(|clip| {
        clip.start() == 0 && clip.duration() == 48_000 && clip.source_offset() == 100
    }));
    assert!(clips.iter().any(|clip| {
        clip.start() == 48_000
            && clip.duration() == 72_000
            && clip.source_offset() == 100
            && clip.source_phase() == 24_000
    }));
    assert!(clips.iter().any(|clip| {
        clip.start() == 144_000
            && clip.duration() == 48_000
            && clip.source_offset() == 100
            && clip.source_phase() == 48_000
    }));
    // The selected interval had 24,000 frames of silence after its source clip;
    // later content still moves by that full rectangle duration.
    assert!(clips.iter().any(|clip| clip.start() == 216_000));
    assert_eq!(
        editor.project().track(other).expect("other").clips()[0],
        other_before
    );
    assert!(editor.undo());
    let mut before = before;
    before.next_id = editor.project().snapshot().next_id;
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn whole_midi_clip_keeps_source_ids_when_constant_tempo_shift_is_exact() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let destination = track(&mut editor, "destination");
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track: destination,
            start: quarter(4),
            length: quarter(4),
        })
        .expect("midi")
    {
        Created::Clip(id) => id,
        other => panic!("{other:?}"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: quarter(0),
            duration: quarter(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("note");
    let original = editor
        .project()
        .track(destination)
        .expect("track")
        .midi_clips()[0]
        .clone();
    let source_identity = original.source_identity();
    let note_id = original.notes()[0].id();
    let clipboard = Arc::new(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![destination], 0, 24_000).expect("range"))
            .expect("empty copy"),
    );

    editor
        .execute(Command::InsertTimeRange {
            clipboard,
            track: destination,
            frame: 48_000,
        })
        .expect("insert");

    let shifted = editor
        .project()
        .track(destination)
        .expect("track")
        .midi_clips()[0]
        .clone();
    assert_eq!(shifted.id(), original.id());
    assert_eq!(shifted.start(), quarter(5));
    assert_eq!(shifted.source_identity(), source_identity);
    assert_eq!(shifted.notes()[0].id(), note_id);
}

#[test]
fn insert_materializes_midi_at_exact_frames_across_tempo_changes() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let destination = track(&mut editor, "destination");
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track: destination,
            start: quarter(0),
            length: quarter(4),
        })
        .expect("midi")
    {
        Created::Clip(id) => id,
        other => panic!("{other:?}"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: quarter(0),
            duration: quarter(4),
            pitch: 64,
            channel: 0,
            velocity: 110,
        })
        .expect("note");
    editor
        .execute(Command::SetTempoPoint {
            beat: quarter(2),
            tempo: Tempo::new(60_000_000).expect("tempo"),
        })
        .expect("tempo point");
    let clipboard = Arc::new(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![destination], 0, 24_000).expect("range"))
            .expect("copy"),
    );
    editor
        .execute(Command::InsertTimeRange {
            clipboard,
            track: destination,
            frame: 24_000,
        })
        .expect("insert");

    let map = editor.project().tempo_map();
    let midi = editor
        .project()
        .track(destination)
        .expect("track")
        .midi_clips();
    let mut frame_spans: Vec<_> = midi
        .iter()
        .map(|clip| {
            let start = map.beat_to_frame(clip.start()).expect("clip start");
            let end = map
                .beat_to_frame(BeatTime::from_raw(clip.start().raw() + clip.length().raw()))
                .expect("clip end");
            (start, end)
        })
        .collect();
    frame_spans.sort_unstable();
    assert_eq!(
        frame_spans,
        vec![(0, 24_000), (24_000, 48_000), (48_000, 168_000)]
    );
    // The right-side gate begins 24,000 frames after the source cut and carries
    // across the tempo point with its original frame-relative endpoint.
    let right = midi
        .iter()
        .find(|clip| map.beat_to_frame(clip.start()).expect("start") == 48_000)
        .expect("right fragment");
    assert_eq!(right.notes()[0].start(), BeatTime::ZERO);
    assert_eq!(
        map.beat_to_frame(BeatTime::from_raw(
            right.start().raw() + right.notes()[0].duration().raw()
        ))
        .expect("gate end"),
        168_000
    );
}

#[test]
fn inserted_midi_loop_carry_becomes_finite_retriggered_gates() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let destination = track(&mut editor, "destination");
    let empty = track(&mut editor, "empty");
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track: destination,
            start: quarter(0),
            length: quarter(4),
        })
        .expect("midi")
    {
        Created::Clip(id) => id,
        other => panic!("{other:?}"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: quarter(0),
            duration: quarter(1),
            pitch: 65,
            channel: 0,
            velocity: 90,
        })
        .expect("note");
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: quarter(1),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: quarter(1),
            looping: true,
            length: quarter(4),
        })
        .expect("loop");
    let clipboard = Arc::new(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![empty], 0, 24_000).expect("range"))
            .expect("empty copy"),
    );

    editor
        .execute(Command::InsertTimeRange {
            clipboard,
            track: destination,
            frame: 24_000,
        })
        .expect("insert");

    let midi = editor
        .project()
        .track(destination)
        .expect("track")
        .midi_clips();
    assert_eq!(midi.len(), 2);
    let left = midi
        .iter()
        .find(|item| item.start() == quarter(0))
        .expect("left loop fragment");
    assert!(left.looping());
    let right = midi
        .iter()
        .find(|item| item.start() == quarter(2))
        .expect("shifted right fragment");
    assert!(!right.looping());
    assert_eq!(right.notes().len(), 3);
    assert!(
        right
            .notes()
            .iter()
            .all(|note| note.duration() == quarter(1))
    );
}

#[test]
fn loop_expansion_limit_refuses_insert_without_mutating_project() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let destination = track(&mut editor, "destination");
    let empty = track(&mut editor, "empty");
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track: destination,
            start: quarter(0),
            length: quarter(4),
        })
        .expect("midi")
    {
        Created::Clip(id) => id,
        other => panic!("{other:?}"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw(1),
            pitch: 67,
            channel: 0,
            velocity: 90,
        })
        .expect("note");
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: BeatTime::from_raw(1),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::from_raw(1),
            looping: true,
            length: quarter(4),
        })
        .expect("dense loop");
    let clipboard = Arc::new(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![empty], 0, 24_000).expect("range"))
            .expect("empty copy"),
    );
    let before = editor.project().snapshot();
    let can_undo_before = editor.can_undo();

    assert!(
        editor
            .execute(Command::InsertTimeRange {
                clipboard,
                track: destination,
                frame: 0,
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    assert_eq!(editor.can_undo(), can_undo_before);
}

#[test]
fn clip_limit_refuses_split_insert_without_mutating_project() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let destination = track(&mut editor, "destination");
    let empty = track(&mut editor, "empty");
    let mut snapshot = editor.project().snapshot();
    let mut clips = Vec::new();
    clips
        .try_reserve_exact(MAX_CLIPS)
        .expect("test fixture capacity");
    clips.push(MidiClipSnapshot {
        content_view: None,
        active: true,
        playback_offset_raw: 0,
        one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
        volume: hex_project::MidiClipVolume::default(),
        name: "clip".into(),
        flavor: hex_project::ClipFlavor::default(),
        id: 5,
        start_raw: 0,
        length_raw: quarter(4).raw(),
        source_length_raw: quarter(4).raw(),
        source_start_raw: 0,
        loop_start_raw: 0,
        loop_end_raw: quarter(4).raw(),
        looping: false,
        notes: Vec::new(),
    });
    for id in 6..(MAX_CLIPS as u64 + 5) {
        clips.push(MidiClipSnapshot {
            content_view: None,
            active: true,
            playback_offset_raw: 0,
            one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
            volume: hex_project::MidiClipVolume::default(),
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            id,
            start_raw: quarter(10 + id - 6).raw(),
            length_raw: quarter(1).raw(),
            source_length_raw: quarter(1).raw(),
            source_start_raw: 0,
            loop_start_raw: 0,
            loop_end_raw: quarter(1).raw(),
            looping: false,
            notes: Vec::new(),
        });
    }
    snapshot.next_id = MAX_CLIPS as u64 + 5;
    let Some(track_snapshot) = snapshot
        .tracks
        .iter_mut()
        .find(|item| item.id == destination.value())
    else {
        panic!("destination snapshot track");
    };
    match &mut track_snapshot.content {
        TrackContentSnapshot::Hybrid { midi_clips, .. } => *midi_clips = clips,
        _ => panic!("hybrid track snapshot"),
    }
    editor = Editor::new(
        Project::from_snapshot(snapshot).expect("maximum project"),
        8,
    )
    .expect("editor");
    let clipboard = Arc::new(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![empty], 0, 24_000).expect("range"))
            .expect("empty copy"),
    );
    let before = editor.project().snapshot();
    let can_undo_before = editor.can_undo();

    assert!(
        editor
            .execute(Command::InsertTimeRange {
                clipboard,
                track: destination,
                frame: 24_000,
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    assert_eq!(editor.can_undo(), can_undo_before);
}

#[test]
fn insert_overflow_and_unavailable_rows_preserve_snapshot_and_prepare_failure_is_atomic() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let first = track(&mut editor, "first");
    let second = track(&mut editor, "second");
    let clipboard = Arc::new(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![first, second], 0, 24_000).expect("range"))
            .expect("copy"),
    );
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::InsertTimeRange {
                clipboard: clipboard.clone(),
                track: second,
                frame: 0,
            })
            .is_err()
    );
    let one_row = Arc::new(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![first], 0, 24_000).expect("range"))
            .expect("copy"),
    );
    assert!(
        editor
            .execute(Command::InsertTimeRange {
                clipboard: one_row.clone(),
                track: first,
                frame: u64::MAX,
            })
            .is_err()
    );
    let rejected: Result<_, Invalid> = editor.execute_prepared(
        Command::InsertTimeRange {
            clipboard: one_row,
            track: first,
            frame: 0,
        },
        |_| Err(Invalid("compile refused")) as Result<(), Invalid>,
    );
    assert!(rejected.is_err());
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn inserting_empty_chunk_into_empty_rows_is_a_history_noop() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let track = track(&mut editor, "empty");
    let clipboard = Arc::new(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![track], 0, 24_000).expect("range"))
            .expect("copy"),
    );
    let before = editor.project().snapshot();
    let can_undo_before = editor.can_undo();
    editor
        .execute(Command::InsertTimeRange {
            clipboard,
            track,
            frame: 48_000,
        })
        .expect("insert empty chunk");
    assert_eq!(editor.project().snapshot(), before);
    assert_eq!(editor.can_undo(), can_undo_before);
}
