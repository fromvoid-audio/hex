use hex_project::{
    ClipId, ClipSnapshot, Command, Created, Editor, MAX_CLIPS, Project, Snapshot,
    TrackContentSnapshot, TrackSnapshot,
};
use std::path::PathBuf;

fn editor_with_clip(start: u64, duration: u64) -> (Editor, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .expect("add track")
    {
        Created::Track(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: PathBuf::from("Samples/Drum.KICK.WAV"),
            sample_rate: 48_000,
            frames: 64,
        })
        .expect("add asset")
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
            source_offset: 7,
            source_length: 11,
            looping: true,
        })
        .expect("place clip")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    (editor, clip)
}

#[test]
fn duplicate_preserves_media_interval_and_undo_redo_identity() {
    let (mut editor, original_id) = editor_with_clip(100, 24);
    let original = editor
        .project()
        .clip(original_id)
        .expect("original")
        .clone();
    let duplicate_id = match editor
        .execute(Command::DuplicateClip(original_id))
        .expect("duplicate")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let duplicate = editor
        .project()
        .clip(duplicate_id)
        .expect("duplicate")
        .clone();
    assert!(duplicate_id.value() > original_id.value());
    assert_eq!(duplicate.asset(), original.asset());
    assert_eq!(duplicate.start(), 124);
    assert_eq!(duplicate.duration(), original.duration());
    assert_eq!(duplicate.source_offset(), original.source_offset());
    assert_eq!(duplicate.source_length(), original.source_length());
    assert_eq!(duplicate.looping(), original.looping());
    assert_eq!(editor.project().assets().count(), 1);

    assert!(editor.undo());
    assert!(editor.project().clip(duplicate_id).is_none());
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert_eq!(
        editor.project().clip(duplicate_id).expect("redone clip"),
        &duplicate
    );
}

#[test]
fn duplicate_repeat_uses_current_source_and_ids_never_rewind_on_branch() {
    let (mut editor, original_id) = editor_with_clip(0, 10);
    let first_duplicate = match editor
        .execute(Command::DuplicateClip(original_id))
        .expect("first duplicate")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    assert!(editor.undo());
    let branch_track = match editor
        .execute(Command::AddTrack {
            name: "branch".into(),
        })
        .expect("branch edit")
    {
        Created::Track(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let second_duplicate = match editor
        .execute(Command::DuplicateClip(original_id))
        .expect("duplicate on branch")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    assert!(second_duplicate.value() > first_duplicate.value());
    assert!(editor.project().clip(first_duplicate).is_none());
    assert_eq!(
        editor
            .project()
            .track(branch_track)
            .expect("branch track")
            .clips()
            .len(),
        0
    );

    // Repeated direct canonical commands that name the same source each place
    // another copy immediately after it. The UI serializes repeats through the
    // returned IDs to build a contiguous chain instead of issuing this pattern.
    let repeated = match editor
        .execute(Command::DuplicateClip(original_id))
        .expect("repeat same source")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    assert!(repeated.value() > second_duplicate.value());
    assert_eq!(
        editor
            .project()
            .clip(repeated)
            .expect("repeated clip")
            .start(),
        10
    );
}

#[test]
fn duplicate_overflow_and_clip_limit_refuse_atomically() {
    let (mut editor, clip) = editor_with_clip(u64::MAX - 1, 1);
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::Gain { track, gain: 0.5 })
        .expect("gain edit");
    assert!(editor.undo());
    let before = editor.project().clone();
    assert!(editor.execute(Command::DuplicateClip(clip)).is_err());
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());

    let clips = (3..3 + MAX_CLIPS as u64)
        .map(|id| ClipSnapshot {
            active: true,
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            edit: None,
            gain: hex_project::ClipGain::UNITY,
            id,
            asset: 1,
            start: id - 3,
            duration: 1,
            source_offset: 0,
            source_length: 1,
            looping: false,
            source_phase: 0,
            fade: hex_project::ClipFade::neutral(1).unwrap(),
        })
        .collect();
    let full = Project::from_legacy_snapshot(Snapshot {
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
        sidechains: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        next_id: 3 + MAX_CLIPS as u64,
        assets: vec![hex_project::AssetSnapshot {
            original_source: None,
            id: 1,
            path: "Samples/One.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 1,
            playback_frames: 1,
            conversion_version: 0,
        }],
        tracks: vec![TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
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
            content: TrackContentSnapshot::Audio(clips),
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
    .expect("valid clip-limit fixture");
    let full_clip = full.tracks()[0]
        .clips()
        .last()
        .expect("last full clip")
        .id();
    let mut full_editor = Editor::new(full, 8).expect("full editor");
    let before = full_editor.project().clone();
    assert!(
        full_editor
            .execute(Command::DuplicateClip(full_clip))
            .is_err()
    );
    assert_eq!(full_editor.project(), &before);
}

#[test]
fn missing_source_clip_is_rejected_without_mutation() {
    let (mut editor, clip) = editor_with_clip(0, 5);
    assert!(editor.undo());
    let before = editor.project().clone();
    assert!(editor.execute(Command::DuplicateClip(clip)).is_err());
    assert_eq!(editor.project(), &before);
}
