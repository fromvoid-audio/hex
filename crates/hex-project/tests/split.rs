use hex_project::{
    AssetSnapshot, ClipId, ClipSnapshot, Command, Created, Editor, MAX_CLIPS, Project, Snapshot,
    TrackContentSnapshot, TrackSnapshot,
};
use std::path::PathBuf;

fn editor_with_clip(
    start: u64,
    duration: u64,
    source_offset: u64,
    source_length: u64,
    looping: bool,
) -> (Editor, ClipId) {
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
            path: PathBuf::from("Samples/One.WAV"),
            sample_rate: 48_000,
            frames: 64,
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
            start,
            duration,
            source_offset,
            source_length,
            looping,
        })
        .expect("clip")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    (editor, clip)
}

#[test]
fn split_partitions_trimmed_one_shot_and_preserves_history_and_identity() {
    let (mut editor, original_id) = editor_with_clip(100, 40, 7, 30, false);
    let original = editor
        .project()
        .clip(original_id)
        .expect("original")
        .clone();
    let right_id = match editor
        .execute(Command::SplitClip {
            clip: original_id,
            frame: 115,
        })
        .expect("split")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let track = &editor.project().tracks()[0];
    assert_eq!(track.clips().len(), 2);
    let left = &track.clips()[0];
    let right = &track.clips()[1];
    assert_eq!(left.id(), original_id);
    assert_eq!(left.start(), 100);
    assert_eq!(left.duration(), 15);
    assert_eq!(left.source_offset(), 7);
    assert_eq!(left.source_length(), 15);
    assert_eq!(right.id(), right_id);
    assert!(right_id.value() > original_id.value());
    assert_eq!(right.start(), 115);
    assert_eq!(right.duration(), 25);
    assert_eq!(right.source_offset(), 22);
    assert_eq!(right.source_length(), 15);
    assert!(!right.looping());
    assert_eq!(
        right.start() + right.duration(),
        original.start() + original.duration()
    );

    let saved_snapshot = editor.project().snapshot();
    let restored = Project::from_snapshot(saved_snapshot.clone()).expect("snapshot load");
    assert_eq!(&restored, editor.project());

    assert!(editor.undo());
    assert!(editor.project().clip(right_id).is_none());
    assert_eq!(editor.project().clip(original_id), Some(&original));
    assert!(editor.redo());
    assert_eq!(editor.project(), &restored);
    let next = match editor
        .execute(Command::AddTrack {
            name: "after split".into(),
        })
        .expect("track after split")
    {
        Created::Track(id) => id.value(),
        other => panic!("unexpected result: {other:?}"),
    };
    assert!(next > right_id.value());
}

#[test]
fn invalid_split_positions_and_limits_leave_state_and_redo_untouched() {
    let (mut editor, clip) = editor_with_clip(10, 12, 4, 5, false);
    editor
        .execute(Command::Gain {
            track: editor.project().tracks()[0].id(),
            gain: 0.5,
        })
        .expect("gain edit");
    assert!(editor.undo());
    let before = editor.project().clone();
    // Frame 15 lies in the silent tail and is a valid cut; edges and outside refuse.
    for frame in [9, 10, 22, 23] {
        assert!(editor.execute(Command::SplitClip { clip, frame }).is_err());
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
    }
    assert!(
        editor
            .execute(Command::SplitClip { clip, frame: 14 })
            .is_ok()
    );

    let (mut looping, looping_clip) = editor_with_clip(0, 20, 0, 8, true);
    looping
        .execute(Command::SplitClip {
            clip: looping_clip,
            frame: 4,
        })
        .expect("looping split");
    let right = looping.project().tracks()[0].clips()[1].id();
    assert_eq!(looping.project().clip(right).unwrap().source_offset(), 0);
    assert_eq!(looping.project().clip(right).unwrap().source_length(), 8);
    assert_eq!(looping.project().clip(right).unwrap().source_phase(), 4);

    let full_project = Project::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        next_id: 3 + MAX_CLIPS as u64,
        assets: vec![AssetSnapshot {
            original_source: None,
            id: 1,
            path: "Samples/One.WAV".into(),
            source_sample_rate: 48_000,
            source_frames: 2,
            playback_frames: 2,
            conversion_version: 0,
        }],
        tracks: vec![TrackSnapshot {
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: 2,
            name: "full".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Audio(
                (0..MAX_CLIPS)
                    .map(|index| ClipSnapshot {
                        active: true,
                        name: "clip".into(),
                        flavor: hex_project::ClipFlavor::default(),
                        edit: None,
                        gain: hex_project::ClipGain::UNITY,
                        id: 3 + index as u64,
                        asset: 1,
                        start: 2 * index as u64,
                        duration: 2,
                        source_offset: 0,
                        source_length: 2,
                        looping: false,
                        source_phase: 0,
                        fade: hex_project::ClipFade::neutral(2).unwrap(),
                    })
                    .collect(),
            ),
            sends: Vec::new(),
        }],
        session: Default::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![hex_project::TempoPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Tempo::DEFAULT,
        )],
        meter_points: vec![hex_project::MeterPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Meter::COMMON_TIME,
        )],
        return_tracks: Vec::new(),
    })
    .expect("valid full project");
    let full_clip = full_project.tracks()[0].clips()[0].id();
    let mut full_editor = Editor::new(full_project, 8).expect("full editor");
    let before = full_editor.project().clone();
    assert!(
        full_editor
            .execute(Command::SplitClip {
                clip: full_clip,
                frame: 1,
            })
            .is_err()
    );
    assert_eq!(full_editor.project(), &before);
}

#[test]
fn missing_clip_split_is_transactional() {
    let (mut editor, clip) = editor_with_clip(0, 8, 0, 8, false);
    assert!(editor.undo());
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::SplitClip { clip, frame: 4 })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
}

#[test]
fn exhausted_id_space_refuses_split_without_state_or_history_changes() {
    let project = Project::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        next_id: u64::MAX,
        assets: vec![AssetSnapshot {
            original_source: None,
            id: 1,
            path: "Samples/One.WAV".into(),
            source_sample_rate: 48_000,
            source_frames: 8,
            playback_frames: 8,
            conversion_version: 0,
        }],
        tracks: vec![TrackSnapshot {
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: 2,
            name: "audio".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
                active: true,
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                edit: None,
                gain: hex_project::ClipGain::UNITY,
                id: 3,
                asset: 1,
                start: 0,
                duration: 8,
                source_offset: 0,
                source_length: 8,
                looping: false,
                source_phase: 0,
                fade: hex_project::ClipFade::neutral(8).unwrap(),
            }]),
            sends: Vec::new(),
        }],
        session: Default::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![hex_project::TempoPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Tempo::DEFAULT,
        )],
        meter_points: vec![hex_project::MeterPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Meter::COMMON_TIME,
        )],
        return_tracks: Vec::new(),
    })
    .expect("valid project with exhausted ID highwater");
    let clip = project.tracks()[0].clips()[0].id();
    let mut editor = Editor::new(project, 8).expect("editor");
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::SplitClip { clip, frame: 4 })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(!editor.can_undo());
    assert!(!editor.can_redo());
}
