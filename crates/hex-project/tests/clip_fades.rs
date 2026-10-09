use hex_project::{ClipFade, ClipId, Command, Created, Editor, Project};
use std::path::PathBuf;

fn editor_with_clip(duration: u64) -> (Editor, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .expect("track")
    {
        Created::Track(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: PathBuf::from("audio.wav"),
            sample_rate: 48_000,
            frames: duration,
        })
        .expect("asset")
    {
        Created::Asset(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let clip = match editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration,
            source_offset: 0,
            source_length: duration,
            looping: false,
        })
        .expect("clip")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    (editor, clip)
}

#[test]
fn endpoint_laws_overlap_single_frames_and_large_domains() {
    let zero = ClipFade::neutral(4).unwrap();
    assert_eq!(
        (0..4).map(|frame| zero.gain_at(frame)).collect::<Vec<_>>(),
        [1.0; 4]
    );

    let fade_in = ClipFade::new(5, 0, 3, 0).unwrap();
    assert_eq!(fade_in.gain_at(0), 0.0);
    assert_eq!(fade_in.gain_at(1), 0.5);
    assert_eq!(fade_in.gain_at(2), 1.0);
    assert_eq!(fade_in.gain_at(4), 1.0);

    let fade_out = ClipFade::new(5, 0, 0, 3).unwrap();
    assert_eq!(fade_out.gain_at(2), 1.0);
    assert_eq!(fade_out.gain_at(3), 0.5);
    assert_eq!(fade_out.gain_at(4), 0.0);

    let overlap = ClipFade::new(4, 0, 4, 4).unwrap();
    assert_eq!(overlap.gain_at(0), 0.0);
    assert!((overlap.gain_at(1) - 2.0 / 9.0).abs() < 1e-7);
    assert_eq!(overlap.gain_at(3), 0.0);

    assert_eq!(ClipFade::new(1, 0, 1, 0).unwrap().gain_at(0), 1.0);
    assert_eq!(ClipFade::new(1, 0, 0, 1).unwrap().gain_at(0), 0.0);

    let huge = ClipFade::new(u64::MAX, 0, 0, u64::MAX).unwrap();
    assert!(huge.gain_at(u64::MAX - 2) > 0.0);
    assert_eq!(huge.gain_at(u64::MAX - 1), 0.0);
}

#[test]
fn malformed_domains_and_fade_commands_are_atomic_and_undoable() {
    assert!(ClipFade::new(0, 0, 0, 0).is_err());
    assert!(ClipFade::new(4, 4, 0, 0).is_err());
    assert!(ClipFade::new(4, u64::MAX, 0, 0).is_err());
    assert!(ClipFade::new(4, 0, 5, 0).is_err());
    assert!(ClipFade::new(4, 0, 0, 5).is_err());
    assert!(
        ClipFade::new(8, 5, 0, 0)
            .unwrap()
            .validate_segment(4)
            .is_err()
    );
    assert!(
        ClipFade::new(8, 1, 0, 0)
            .unwrap()
            .validate_segment(u64::MAX)
            .is_err()
    );

    let (valid, existing_clip) = editor_with_clip(8);
    let mut snapshot = valid.project().snapshot();
    snapshot.tracks[0].audio_clips_mut().unwrap()[0].fade = ClipFade::new(8, 7, 0, 0).unwrap();
    assert!(Project::from_snapshot(snapshot).is_err());
    let mut snapshot = valid.project().snapshot();
    snapshot.tracks[0].audio_clips_mut().unwrap()[0].duration = u64::MAX;
    snapshot.tracks[0].audio_clips_mut().unwrap()[0].fade =
        ClipFade::new(u64::MAX, 1, 0, 0).unwrap();
    assert!(Project::from_snapshot(snapshot).is_err());
    assert!(valid.project().clip(existing_clip).is_some());

    let (mut editor, clip) = editor_with_clip(8);
    let original = editor.project().clip(clip).unwrap().fade();
    let before = editor.project().clone();
    let can_undo = editor.can_undo();
    assert!(
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 9,
                fade_out: 0,
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.project().clip(clip).unwrap().fade(), original);
    assert_eq!(editor.can_undo(), can_undo);

    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 7,
            fade_out: 6,
        })
        .expect("set fades");
    assert_eq!(
        editor.project().clip(clip).unwrap().fade(),
        ClipFade::new(8, 0, 7, 6).unwrap()
    );
    assert!(editor.undo());
    assert_eq!(editor.project().clip(clip).unwrap().fade(), original);
    assert!(editor.redo());
    assert_eq!(editor.project().clip(clip).unwrap().fade().fade_in(), 7);
}

#[test]
fn duplicate_and_move_preserve_domain_while_trim_reanchors_and_clamps() {
    let (mut editor, clip) = editor_with_clip(8);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 6,
            fade_out: 5,
        })
        .unwrap();
    let original_fade = editor.project().clip(clip).unwrap().fade();
    let duplicate = match editor.execute(Command::DuplicateClip(clip)).unwrap() {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    assert_eq!(
        editor.project().clip(duplicate).unwrap().fade(),
        original_fade
    );
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip: duplicate,
            track,
            start: 20,
        })
        .unwrap();
    assert_eq!(
        editor.project().clip(duplicate).unwrap().fade(),
        original_fade
    );

    editor
        .execute(Command::TrimClip {
            clip,
            start: 0,
            duration: 4,
            source_offset: 0,
            source_length: 4,
        })
        .unwrap();
    assert_eq!(
        editor.project().clip(clip).unwrap().fade(),
        ClipFade::new(4, 0, 4, 4).unwrap()
    );
}

#[test]
fn recursive_split_preserves_every_sample_of_original_envelope() {
    let (mut editor, clip) = editor_with_clip(11);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 7,
            fade_out: 6,
        })
        .unwrap();
    let original = editor.project().clip(clip).unwrap().fade();
    editor
        .execute(Command::SplitClip { clip, frame: 4 })
        .unwrap();
    let right = editor.project().tracks()[0].clips()[1].id();
    editor
        .execute(Command::SplitClip {
            clip: right,
            frame: 8,
        })
        .unwrap();
    let segments = editor.project().tracks()[0].clips();
    assert_eq!(segments.len(), 3);
    let mut absolute = 0;
    for segment in segments {
        assert_eq!(segment.fade().span(), 11);
        assert_eq!(segment.fade().offset(), absolute);
        for local in 0..segment.duration() {
            assert_eq!(
                segment.fade().gain_at(local),
                original.gain_at(absolute + local)
            );
        }
        absolute += segment.duration();
    }
    assert_eq!(absolute, 11);
    assert!(editor.undo());
    assert!(editor.undo());
    assert_eq!(editor.project().clip(clip).unwrap().fade(), original);
    assert!(editor.redo());
    assert!(editor.redo());
}
