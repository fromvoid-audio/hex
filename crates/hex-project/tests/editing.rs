use hex_project::{AssetId, ClipId, Command, Created, Editor, Project, TrackId};

fn editor(capacity: usize) -> Editor {
    Editor::new(Project::new(48_000).unwrap(), capacity).unwrap()
}
fn track(e: &mut Editor, name: &str) -> TrackId {
    match e.execute(Command::AddTrack { name: name.into() }).unwrap() {
        Created::Track(id) => id,
        _ => panic!("track id required"),
    }
}
fn asset(e: &mut Editor) -> AssetId {
    match e
        .execute(Command::AddAsset {
            path: "Media/Original.WAV".into(),
            sample_rate: 48_000,
            frames: 100,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        _ => panic!("asset id required"),
    }
}
fn clip(e: &mut Editor, t: TrackId, a: AssetId) -> ClipId {
    match e
        .execute(Command::PlaceClip {
            track: t,
            asset: a,
            start: 10,
            duration: 50,
            source_offset: 5,
            source_length: 20,
            looping: true,
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        _ => panic!("clip id required"),
    }
}
#[test]
fn move_trim_and_history_restore_complete_content() {
    let mut e = editor(16);
    let a = asset(&mut e);
    let t = track(&mut e, "Original Case");
    let destination = track(&mut e, "destination");
    let c = clip(&mut e, t, a);
    e.execute(Command::MoveClip {
        clip: c,
        track: destination,
        start: 25,
    })
    .unwrap();
    assert!(e.project().track(t).unwrap().clips().is_empty());
    assert_eq!(e.project().track(destination).unwrap().clips()[0].id(), c);
    e.execute(Command::TrimClip {
        clip: c,
        start: 27,
        duration: 10,
        source_offset: 9,
        source_length: 8,
    })
    .unwrap();
    assert_eq!(e.project().clip(c).unwrap().source_offset(), 9);
    assert!(e.undo());
    assert_eq!(e.project().clip(c).unwrap().start(), 25);
    assert!(e.undo());
    assert_eq!(e.project().track(t).unwrap().clips()[0].id(), c);
    assert!(e.redo());
    assert!(e.redo());
    assert_eq!(e.project().clip(c).unwrap().duration(), 10);
    assert_eq!(
        e.project().asset(a).unwrap().path().to_str(),
        Some("Media/Original.WAV")
    );
    assert_eq!(e.project().track(t).unwrap().name(), "Original Case");
}
#[test]
fn rejected_edits_leave_project_and_redo_intact() {
    let mut e = editor(16);
    let a = asset(&mut e);
    let t = track(&mut e, "one");
    let c = clip(&mut e, t, a);
    e.execute(Command::Gain {
        track: t,
        gain: 0.5,
    })
    .unwrap();
    assert!(e.undo());
    let before = e.project().clone();
    for cmd in [
        Command::Gain {
            track: t,
            gain: f32::NAN,
        },
        Command::MoveClip {
            clip: c,
            track: t,
            start: u64::MAX,
        },
        Command::TrimClip {
            clip: c,
            start: 1,
            duration: 10,
            source_offset: 99,
            source_length: 2,
        },
        Command::RemoveAsset(a),
        Command::AddAsset {
            path: "mismatch.wav".into(),
            sample_rate: 44_100,
            frames: 10,
        },
    ] {
        assert!(e.execute(cmd).is_err());
        assert_eq!(e.project(), &before);
        assert!(e.can_redo());
    }
    assert!(e.redo());
    assert_eq!(e.project().track(t).unwrap().gain(), 0.5);
}
#[test]
fn ids_never_reuse_after_undo_branch_and_stale_commands_fail() {
    let mut e = editor(8);
    let first = track(&mut e, "first");
    assert!(e.undo());
    let next = track(&mut e, "next");
    assert!(next.value() > first.value());
    assert!(!e.can_redo());
    let before = e.project().clone();
    assert!(e.execute(Command::RemoveTrack(first)).is_err());
    assert_eq!(e.project(), &before);
}
#[test]
fn deleting_track_restores_owned_clips_and_asset_references() {
    let mut e = editor(16);
    let a = asset(&mut e);
    let t = track(&mut e, "one");
    let c = clip(&mut e, t, a);
    e.execute(Command::RemoveTrack(t)).unwrap();
    assert!(e.project().clip(c).is_none());
    assert!(e.project().asset(a).is_some());
    assert!(e.undo());
    assert_eq!(e.project().clip(c).unwrap().asset(), a);
    e.execute(Command::RemoveClip(c)).unwrap();
    e.execute(Command::RemoveAsset(a)).unwrap();
    assert!(e.project().asset(a).is_none());
    assert!(e.undo());
    assert!(e.undo());
    assert!(e.project().clip(c).is_some());
}
#[test]
fn history_capacity_and_noop_do_not_lose_redo() {
    let mut e = editor(2);
    let t = track(&mut e, "one");
    e.execute(Command::Gain {
        track: t,
        gain: 0.5,
    })
    .unwrap();
    e.execute(Command::Mute {
        track: t,
        muted: true,
    })
    .unwrap();
    assert!(e.undo());
    e.execute(Command::Mute {
        track: t,
        muted: false,
    })
    .unwrap();
    assert!(e.can_redo());
    assert!(e.redo());
    assert!(e.undo());
    assert!(e.undo());
    assert!(!e.undo());
    assert!(e.project().track(t).is_some());
    assert!(Editor::new(Project::new(48_000).unwrap(), 0).is_err());
}
#[test]
fn source_and_timeline_intervals_are_checked() {
    let mut e = editor(8);
    let a = asset(&mut e);
    let t = track(&mut e, "one");
    for (start, duration, offset, length) in [
        (0, 0, 0, 10),
        (u64::MAX, 1, 0, 10),
        (0, 10, u64::MAX, 2),
        (0, 10, 90, 11),
        (0, 10, 0, 0),
    ] {
        let before = e.project().clone();
        assert!(
            e.execute(Command::PlaceClip {
                track: t,
                asset: a,
                start,
                duration,
                source_offset: offset,
                source_length: length,
                looping: false
            })
            .is_err()
        );
        assert_eq!(e.project(), &before);
    }
    assert!(e.execute(Command::AddTrack { name: "\n".into() }).is_err());
    assert!(Project::new(0).is_err());
}

#[test]
fn unchanged_move_preserves_order_and_redo_branch() {
    let mut e = editor(16);
    let a = asset(&mut e);
    let t = track(&mut e, "one");
    let first = clip(&mut e, t, a);
    let _second = e
        .execute(Command::PlaceClip {
            track: t,
            asset: a,
            start: 60,
            duration: 50,
            source_offset: 5,
            source_length: 20,
            looping: true,
        })
        .unwrap();
    e.execute(Command::Gain {
        track: t,
        gain: 0.5,
    })
    .unwrap();
    assert!(e.undo());
    let before = e.project().clone();
    e.execute(Command::MoveClip {
        clip: first,
        track: t,
        start: 10,
    })
    .unwrap();
    assert_eq!(e.project(), &before);
    assert!(e.can_redo());
}
