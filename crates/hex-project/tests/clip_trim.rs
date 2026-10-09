use hex_project::{Command, Created, Editor, Project, TrimEdge};
use std::path::PathBuf;

fn editor_with_clip(
    looping: bool,
    start: u64,
    duration: u64,
    offset: u64,
    length: u64,
    asset: u64,
) -> (Editor, hex_project::ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: PathBuf::from("audio.wav"),
            sample_rate: 48_000,
            frames: asset,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let clip = match editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start,
            duration,
            source_offset: offset,
            source_length: length,
            looping,
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    (editor, clip)
}

#[test]
fn loop_split_keeps_source_region_and_advances_phase_recursively() {
    let (mut editor, clip) = editor_with_clip(true, 10, 11, 2, 3, 8);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 8,
            fade_out: 7,
        })
        .unwrap();
    editor
        .execute(Command::SplitClip { clip, frame: 14 })
        .unwrap();
    let right = editor.project().tracks()[0].clips()[1].id();
    let right_clip = editor.project().clip(right).unwrap();
    assert_eq!(
        (
            right_clip.source_offset(),
            right_clip.source_length(),
            right_clip.source_phase()
        ),
        (2, 3, 1)
    );
    editor
        .execute(Command::SplitClip {
            clip: right,
            frame: 18,
        })
        .unwrap();
    let clips = editor.project().tracks()[0].clips();
    assert_eq!(
        clips
            .iter()
            .map(|clip| clip.source_phase())
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert!(
        clips
            .iter()
            .all(|clip| clip.source_offset() == 2 && clip.source_length() == 3)
    );
    assert_eq!(
        clips
            .iter()
            .map(|clip| clip.fade().offset())
            .collect::<Vec<_>>(),
        [0, 4, 8]
    );
    let duplicate_source = clips[1].id();
    let duplicate = match editor
        .execute(Command::DuplicateClip(duplicate_source))
        .unwrap()
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    assert_eq!(editor.project().clip(duplicate).unwrap().source_phase(), 1);
}

#[test]
fn edge_trim_preserves_one_shot_silent_tail_and_clamps_to_legal_edges() {
    let (mut editor, clip) = editor_with_clip(false, 10, 8, 4, 5, 20);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 6,
            fade_out: 5,
        })
        .unwrap();
    let prior = editor.project().clip(clip).unwrap().clone();
    let preview = prior.preview_edge_trim(TrimEdge::Left, 8, 20).unwrap();
    assert_eq!(
        (
            preview.start(),
            preview.duration(),
            preview.source_offset(),
            preview.source_length()
        ),
        (8, 10, 2, 7)
    );
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 8,
        })
        .unwrap();
    let trimmed = editor.project().clip(clip).unwrap().clone();
    assert_eq!(
        (
            trimmed.start(),
            trimmed.duration(),
            trimmed.source_offset(),
            trimmed.source_length()
        ),
        (8, 10, 2, 7)
    );
    assert_eq!(trimmed.fade().span(), 10);
    assert_eq!(trimmed.fade().offset(), 0);
    let left_bound = prior.preview_edge_trim(TrimEdge::Left, 0, 20).unwrap();
    assert_eq!((left_bound.start(), left_bound.source_offset()), (6, 0));
    let right_bound = trimmed
        .preview_edge_trim(TrimEdge::Right, u64::MAX, 20)
        .unwrap();
    assert_eq!(
        (
            right_bound.start() + right_bound.duration(),
            right_bound.source_length()
        ),
        (29, 18)
    );
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: 15,
        })
        .unwrap();
    let right = editor.project().clip(clip).unwrap();
    assert_eq!((right.duration(), right.source_length()), (7, 4));
    assert_eq!(right.duration() - right.source_length(), 3);
    assert_eq!(right.source_offset(), 2);
    assert!(editor.undo());
    assert_eq!(editor.project().clip(clip).unwrap(), &trimmed);
}

#[test]
fn looping_left_trim_and_extension_update_phase_and_right_trim_keeps_region() {
    let (mut editor, clip) = editor_with_clip(true, 10, 8, 2, 3, 8);
    editor
        .execute(Command::SplitClip { clip, frame: 14 })
        .unwrap();
    let right = editor.project().tracks()[0].clips()[1].id();
    editor
        .execute(Command::TrimClipEdge {
            clip: right,
            edge: TrimEdge::Left,
            frame: 15,
        })
        .unwrap();
    assert_eq!(editor.project().clip(right).unwrap().source_phase(), 2);
    editor
        .execute(Command::TrimClipEdge {
            clip: right,
            edge: TrimEdge::Left,
            frame: 14,
        })
        .unwrap();
    assert_eq!(editor.project().clip(right).unwrap().source_phase(), 1);
    editor
        .execute(Command::TrimClipEndBy {
            clip: right,
            frames: -1,
        })
        .unwrap();
    assert_eq!(editor.project().clip(right).unwrap().duration(), 3);
    editor
        .execute(Command::TrimClipEndBy {
            clip: right,
            frames: 3,
        })
        .unwrap();
    let extended = editor.project().clip(right).unwrap();
    assert_eq!(extended.duration(), 6);
    assert_eq!(
        (
            extended.source_offset(),
            extended.source_length(),
            extended.source_phase()
        ),
        (2, 3, 1)
    );
    let (start, duration, source_length) = (
        extended.start(),
        extended.duration(),
        extended.source_length(),
    );
    editor
        .execute(Command::TrimClip {
            clip: right,
            start,
            duration,
            source_offset: 1,
            source_length,
        })
        .unwrap();
    assert_eq!(editor.project().clip(right).unwrap().source_phase(), 0);
}

#[test]
fn edge_trim_preview_noop_retains_fade_and_bounds_wrap_without_overflow() {
    let (mut editor, clip) = editor_with_clip(true, u64::MAX - 8, 8, 0, 3, 3);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 3,
            fade_out: 2,
        })
        .unwrap();
    let original = editor.project().clip(clip).unwrap().clone();
    let unchanged = original
        .preview_edge_trim(TrimEdge::Left, u64::MAX - 8, 3)
        .unwrap();
    assert_eq!(unchanged, original);
    let right = original
        .preview_edge_trim(TrimEdge::Right, u64::MAX, 3)
        .unwrap();
    assert_eq!(right.start() + right.duration(), u64::MAX);
    assert_eq!(right.duration(), 8);
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: u64::MAX,
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap(), &original);
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::Mute { track, muted: true })
        .unwrap();
    assert!(editor.undo());
    assert!(editor.can_redo());
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: u64::MAX,
        })
        .unwrap();
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert!(editor.project().track(track).unwrap().muted());
}

#[test]
fn invalid_snapshot_phases_and_commands_are_rejected_without_history_changes() {
    let (mut editor, clip) = editor_with_clip(true, 0, 4, 0, 3, 3);
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0].audio_clips_mut().unwrap()[0].source_phase = 3;
    assert!(Project::from_snapshot(snapshot).is_err());
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0].audio_clips_mut().unwrap()[0].looping = false;
    snapshot.tracks[0].audio_clips_mut().unwrap()[0].source_phase = 1;
    assert!(Project::from_snapshot(snapshot).is_err());
    let before = editor.project().clone();
    let undo = editor.can_undo();
    assert!(
        editor
            .execute(Command::TrimClip {
                clip,
                start: 0,
                duration: 4,
                source_offset: 0,
                source_length: 0
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.can_undo(), undo);
    editor.execute(Command::RemoveClip(clip)).unwrap();
    let removed = editor.project().clone();
    let history = editor.can_undo();
    assert!(
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 0,
            })
            .is_err()
    );
    assert_eq!(editor.project(), &removed);
    assert_eq!(editor.can_undo(), history);
}
