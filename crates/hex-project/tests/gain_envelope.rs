use hex_project::{
    BeatTime, Command, Created, Editor, GainInterpolation, GainPoint, MAX_GAIN_POINTS_PER_TRACK,
    Project, TimeRange,
};
fn point(raw: u64, gain: f32) -> GainPoint {
    GainPoint {
        incoming_gain: None,
        beat: BeatTime::from_raw(raw),
        gain,
        outgoing: GainInterpolation::Linear,
    }
}
fn fixture() -> (Editor, hex_project::TrackId) {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "gain".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    (editor, track)
}
#[test]
fn edits_history_identity_owner_removal_and_manual_refusal_are_atomic() {
    let (mut e, track) = fixture();
    let next_id = e.project().snapshot().next_id;
    e.execute(Command::ReplaceTrackGainEnvelope {
        track,
        points: vec![point(0, 0.25), point(10, 0.75)],
    })
    .unwrap();
    assert_eq!(e.project().snapshot().next_id, next_id);
    e.execute(Command::UpsertTrackGainPoint {
        track,
        point: point(10, 1.0),
    })
    .unwrap();
    let before = e.project().clone();
    e.execute(Command::MoveTrackGainPoint {
        track,
        from: BeatTime::ZERO,
        to: BeatTime::ZERO,
    })
    .unwrap();
    assert_eq!(e.project(), &before);
    for command in [
        Command::MoveTrackGainPoint {
            track,
            from: BeatTime::ZERO,
            to: BeatTime::from_raw(10),
        },
        Command::Gain { track, gain: 1.5 },
        Command::GainBy { track, amount: 0.1 },
        Command::SplitTracksAtFrame {
            tracks: vec![track],
            frame: 4,
        },
        Command::DeleteTimeRange {
            range: TimeRange::new(vec![track], 0, 4).unwrap(),
        },
    ] {
        assert!(e.execute(command).is_err());
        assert_eq!(e.project(), &before);
    }
    assert!(
        e.project()
            .capture_time_range(&TimeRange::new(vec![track], 0, 4).unwrap())
            .is_err()
    );
    e.execute(Command::MoveTrackGainPoint {
        track,
        from: BeatTime::ZERO,
        to: BeatTime::from_raw(5),
    })
    .unwrap();
    assert_eq!(
        e.project().track(track).unwrap().gain_envelope()[0]
            .beat
            .raw(),
        5
    );
    e.execute(Command::RemoveTrack(track)).unwrap();
    assert!(!e.project().has_gain_automation());
    assert!(e.undo());
    assert_eq!(e.project().track(track).unwrap().gain_envelope().len(), 2);
    e.execute(Command::RemoveTrackGainPoint {
        track,
        beat: BeatTime::from_raw(5),
    })
    .unwrap();
    assert_eq!(e.project().track(track).unwrap().gain_envelope().len(), 1);
    e.execute(Command::ClearTrackGainEnvelope { track })
        .unwrap();
    e.execute(Command::Gain { track, gain: 0.5 }).unwrap();
    assert!(!e.project().has_gain_automation());
}
#[test]
fn malformed_points_track_and_global_caps_preserve_history_and_redo() {
    let (mut e, track) = fixture();
    e.execute(Command::Gain { track, gain: 0.25 }).unwrap();
    assert!(e.undo());
    let before = e.project().clone();
    for points in [
        vec![point(0, f32::NAN)],
        vec![point(0, -0.1)],
        vec![point(0, 2.01)],
        vec![point(2, 1.0), point(1, 1.0)],
        vec![point(1, 1.0), point(1, 1.0)],
        vec![point(0, 1.0); MAX_GAIN_POINTS_PER_TRACK + 1],
    ] {
        assert!(
            e.execute(Command::ReplaceTrackGainEnvelope { track, points })
                .is_err()
        );
        assert_eq!(e.project(), &before);
        assert!(e.can_redo());
    }
    let mut snapshot = before.snapshot();
    let lane: Vec<_> = (0..1024).map(|i| point(i, 1.0)).collect();
    let mut owners = Editor::new(before, 1).unwrap();
    for _ in 0..64 {
        owners
            .execute(Command::AddTrack { name: "cap".into() })
            .unwrap();
    }
    snapshot.tracks = owners.project().snapshot().tracks;
    snapshot.next_id = owners.project().snapshot().next_id;
    for track in &mut snapshot.tracks[..64] {
        track.gain_envelope = lane.clone();
    }
    let last = owners.project().tracks()[64].id();
    let project = Project::from_snapshot(snapshot).unwrap();
    let mut e = Editor::new(project.clone(), 1).unwrap();
    assert!(
        e.execute(Command::ReplaceTrackGainEnvelope {
            track: last,
            points: vec![point(0, 1.0)]
        })
        .is_err()
    );
    assert_eq!(e.project(), &project);
    assert!(
        e.execute(Command::UpsertTrackGainPoint {
            track,
            point: point(2048, 1.0)
        })
        .is_err()
    );
    assert_eq!(e.project(), &project);
}
#[test]
fn clip_only_edits_keep_lane_anchors_and_affected_paste_insert_refuse() {
    let (mut e, track) = fixture();
    let Created::Asset(asset) = e
        .execute(Command::AddAsset {
            path: "memory.wav".into(),
            sample_rate: 48000,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Clip(clip) = e
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 8,
            source_offset: 0,
            source_length: 8,
            looping: false,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let range = TimeRange::new(vec![track], 0, 8).unwrap();
    let clipboard = std::sync::Arc::new(e.project().capture_time_range(&range).unwrap());
    e.execute(Command::ReplaceTrackGainEnvelope {
        track,
        points: vec![point(1, 0.5), point(9, 1.0)],
    })
    .unwrap();
    let lane = e.project().track(track).unwrap().gain_envelope().to_vec();
    e.execute(Command::MoveClip {
        clip,
        track,
        start: 3,
    })
    .unwrap();
    assert_eq!(e.project().track(track).unwrap().gain_envelope(), lane);
    let before = e.project().clone();
    for command in [
        Command::PasteTimeRange {
            clipboard: std::sync::Arc::clone(&clipboard),
            track,
            frame: 0,
        },
        Command::InsertTimeRange {
            clipboard,
            track,
            frame: 0,
        },
    ] {
        assert!(e.execute(command).is_err());
        assert_eq!(e.project(), &before);
    }
}
