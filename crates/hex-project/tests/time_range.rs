use hex_project::{BeatTime, Command, Created, Editor, Invalid, Project, TimeRange, TrackId};
use std::sync::Arc;

fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).expect("beat")
}
fn track(editor: &mut Editor, name: &str) -> TrackId {
    match editor
        .execute(Command::AddTrack { name: name.into() })
        .expect("track")
    {
        Created::Track(id) => id,
        other => panic!("{other:?}"),
    }
}
fn fixture() -> (Editor, TrackId, TrackId) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    let a = track(&mut editor, "a");
    let b = track(&mut editor, "b");
    let asset = match editor
        .execute(Command::AddAsset {
            path: "Audio/original.wav".into(),
            sample_rate: 48_000,
            frames: 240_000,
        })
        .expect("asset")
    {
        Created::Asset(id) => id,
        other => panic!("{other:?}"),
    };
    editor
        .execute(Command::PlaceClip {
            track: a,
            asset,
            start: 0,
            duration: 96_000,
            source_offset: 100,
            source_length: 120_000,
            looping: false,
        })
        .expect("audio");
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track: b,
            start: beat(0),
            length: beat(4),
        })
        .expect("midi")
    {
        Created::Clip(id) => id,
        other => panic!("{other:?}"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(0),
            duration: beat(4),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("note");
    (editor, a, b)
}

#[test]
fn cut_mixed_range_preserves_tails_and_one_undo_restores_exact_state() {
    let (mut editor, a, b) = fixture();
    let mut before = editor.project().snapshot();
    editor
        .execute(Command::DeleteTimeRange {
            range: TimeRange::new(vec![a, b], 24_000, 72_000).expect("range"),
        })
        .expect("cut");
    let audio = editor.project().track(a).expect("track").clips();
    assert_eq!(audio.len(), 2);
    assert_eq!(
        (
            audio[0].start(),
            audio[0].duration(),
            audio[0].source_offset()
        ),
        (0, 24_000, 100)
    );
    assert_eq!(
        (
            audio[1].start(),
            audio[1].duration(),
            audio[1].source_offset()
        ),
        (72_000, 24_000, 72_100)
    );
    let midi = editor.project().track(b).expect("track").midi_clips();
    assert_eq!(midi.len(), 2);
    assert_eq!(midi[1].notes()[0].start(), BeatTime::ZERO);
    let after = editor.project().snapshot();
    assert!(editor.undo());
    before.next_id = editor.project().snapshot().next_id;
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
    Project::from_snapshot(after).expect("round trip");
}

#[test]
fn paste_uses_fresh_ids_and_replaces_silence_in_the_whole_rectangle() {
    let (mut editor, a, b) = fixture();
    let original = editor.project().track(a).expect("track").clips()[0].clone();
    let range = TimeRange::new(vec![a, b], 24_000, 120_000).expect("range");
    let copied = Arc::new(editor.project().capture_time_range(&range).expect("copy"));
    editor
        .execute(Command::PasteTimeRange {
            clipboard: copied,
            track: a,
            frame: 0,
        })
        .expect("paste");
    let audio = editor.project().track(a).expect("track").clips();
    assert_eq!(audio.len(), 1);
    assert_ne!(audio[0].id(), original.id());
    assert_eq!(
        (
            audio[0].start(),
            audio[0].duration(),
            audio[0].source_offset()
        ),
        (0, 72_000, 24_100)
    );
    let midi = editor.project().track(b).expect("track").midi_clips();
    assert_eq!(midi.len(), 1);
    assert!(!midi[0].looping());
    assert_eq!(midi[0].notes()[0].duration(), beat(3));
}

#[test]
fn copy_time_range_to_copies_fragments_in_one_step_and_replaces_only_the_destination() {
    let (mut editor, audio_track, midi_track) = fixture();
    let source = editor.project().track(audio_track).unwrap().clips()[0].clone();
    let range = TimeRange::new(vec![audio_track], 24_000, 72_000).expect("range");
    editor
        .execute(Command::CopyTimeRangeTo {
            range: range.clone(),
            track: audio_track,
            frame: 48_000,
        })
        .expect("copy time range");
    let clips = editor.project().track(audio_track).unwrap().clips();
    assert_eq!(clips.len(), 2);
    assert!(clips.iter().any(|clip| {
        clip.id() == source.id() && clip.start() == 0 && clip.duration() == 48_000
    }));
    assert!(clips.iter().any(|clip| {
        clip.id() != source.id()
            && clip.start() == 48_000
            && clip.duration() == 48_000
            && clip.source_offset() == source.source_offset() + 24_000
    }));
    assert_eq!(
        editor.project().track(midi_track).unwrap().midi_clips()[0].start(),
        beat(0),
        "unselected rows remain unchanged"
    );
    assert!(editor.undo());
    assert_eq!(
        editor.project().track(audio_track).unwrap().clips()[0],
        source
    );
}

#[test]
fn copy_time_range_at_its_origin_is_a_noop_that_keeps_redo() {
    let (mut editor, audio_track, _) = fixture();
    let range = TimeRange::new(vec![audio_track], 24_000, 72_000).expect("range");
    editor
        .execute(Command::AddTrack {
            name: "temporary".into(),
        })
        .expect("track");
    assert!(editor.undo());
    assert!(editor.can_redo());
    editor
        .execute(Command::CopyTimeRangeTo {
            range,
            track: audio_track,
            frame: 24_000,
        })
        .expect("no-op copy");
    assert!(editor.can_redo());
}

#[test]
fn overlapping_move_captures_before_erasing_and_is_atomic() {
    let (mut editor, a, b) = fixture();
    let mut before = editor.project().snapshot();
    editor
        .execute(Command::MoveTimeRange {
            range: TimeRange::new(vec![a, b], 24_000, 72_000).expect("range"),
            track: a,
            frame: 48_000,
        })
        .expect("move");
    let audio = editor.project().track(a).expect("track").clips();
    assert_eq!(audio.len(), 2);
    assert!(
        audio
            .iter()
            .any(|c| c.start() == 48_000 && c.duration() == 48_000 && c.source_offset() == 24_100)
    );
    assert!(editor.undo());
    before.next_id = editor.project().snapshot().next_id;
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn malformed_destination_and_prepare_failure_preserve_project_and_redo() {
    let (mut editor, a, b) = fixture();
    let range = TimeRange::new(vec![a, b], 24_000, 72_000).expect("range");
    let clipboard = Arc::new(editor.project().capture_time_range(&range).expect("copy"));
    editor
        .execute(Command::DeleteTimeRange {
            range: range.clone(),
        })
        .expect("delete");
    assert!(editor.undo());
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::PasteTimeRange {
                clipboard: clipboard.clone(),
                track: b,
                frame: 0
            })
            .is_err()
    );
    assert!(
        editor
            .execute(Command::PasteTimeRange {
                clipboard,
                track: a,
                frame: u64::MAX
            })
            .is_err()
    );
    let rejected: Result<_, Invalid> = editor
        .execute_prepared(Command::DeleteTimeRange { range }, |_| {
            Err(Invalid("compile refused")) as Result<(), Invalid>
        });
    assert!(rejected.is_err());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert!(TimeRange::new(vec![a, a], 0, 1).is_err());
    assert!(TimeRange::new(vec![a], 1, 1).is_err());
    assert!(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![b, a], 0, 1).expect("shape"))
            .is_err()
    );
}

#[test]
fn moving_to_same_origin_is_a_noop_that_keeps_redo() {
    let (mut editor, a, b) = fixture();
    let range = TimeRange::new(vec![a, b], 0, 96_000).expect("range");
    editor
        .execute(Command::DeleteTimeRange {
            range: range.clone(),
        })
        .expect("delete");
    assert!(editor.undo());
    let before = editor.project().snapshot();
    editor
        .execute(Command::MoveTimeRange {
            range,
            track: a,
            frame: 0,
        })
        .expect("noop");
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
}

#[test]
fn midi_paste_keeps_frame_alignment_across_a_tempo_change() {
    let (mut editor, a, b) = fixture();
    editor
        .execute(Command::SetTempoPoint {
            beat: beat(4),
            tempo: hex_project::Tempo::new(60_000_000).expect("tempo"),
        })
        .expect("tempo point");
    let clipboard = Arc::new(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![b], 24_000, 72_000).expect("range"))
            .expect("capture"),
    );
    editor
        .execute(Command::PasteTimeRange {
            clipboard,
            track: a,
            frame: 96_000,
        })
        .expect("paste");
    let clip = &editor.project().track(a).expect("track").midi_clips()[0];
    assert_eq!(clip.start(), beat(4));
    assert_eq!(clip.length(), beat(1));
    assert_eq!(clip.notes()[0].duration(), beat(1));
}

#[test]
fn repeated_midi_source_is_materialized_as_finite_selected_gates() {
    let (mut editor, a, b) = fixture();
    let clip = editor.project().track(b).expect("track").midi_clips()[0].id();
    // A two-beat source gate loops four times in an eight-beat arrangement clip.
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: beat(0),
            loop_start: beat(0),
            loop_end: beat(2),
            looping: true,
            length: beat(8),
        })
        .expect("loop");
    let clipboard = Arc::new(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![b], 24_000, 120_000).expect("range"))
            .expect("capture"),
    );
    editor
        .execute(Command::PasteTimeRange {
            clipboard,
            track: a,
            frame: 192_000,
        })
        .expect("paste");
    let copied = &editor.project().track(a).expect("track").midi_clips()[0];
    assert!(!copied.looping());
    assert_eq!(copied.notes().len(), 3);
    assert_eq!(copied.notes()[0].start(), BeatTime::ZERO);
    assert_eq!(copied.length(), beat(4));
    assert!(
        copied
            .notes()
            .iter()
            .all(|n| n.start().raw() + n.duration().raw() <= copied.length().raw())
    );
}

#[test]
fn pasted_audio_inherits_the_selected_fade_domain() {
    let (mut editor, a, _) = fixture();
    let original = editor.project().track(a).expect("track").clips()[0].id();
    editor
        .execute(Command::SetClipFades {
            clip: original,
            fade_in: 48_000,
            fade_out: 24_000,
        })
        .expect("fades");
    let clipboard = Arc::new(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![a], 24_000, 72_000).expect("range"))
            .expect("copy"),
    );
    editor
        .execute(Command::PasteTimeRange {
            clipboard,
            track: a,
            frame: 120_000,
        })
        .expect("paste");
    let pasted = editor
        .project()
        .track(a)
        .expect("track")
        .clips()
        .iter()
        .find(|c| c.start() == 120_000)
        .expect("pasted");
    assert_eq!(pasted.fade().offset(), 24_000);
    assert_eq!(pasted.fade().span(), 96_000);
    assert_eq!(pasted.fade().fade_in(), 48_000);
    assert_eq!(pasted.fade().fade_out(), 24_000);
}

#[test]
fn bounded_loop_capture_refuses_excessive_occurrences_without_editing() {
    let (mut editor, _, b) = fixture();
    let clip = editor.project().track(b).expect("track").midi_clips()[0].id();
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::from_raw(1),
            looping: true,
            length: beat(4),
        })
        .expect("canonical metadata permits dense loop");
    let before = editor.project().snapshot();
    assert!(
        editor
            .project()
            .capture_time_range(&TimeRange::new(vec![b], 0, 24_000).expect("range"))
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
}
