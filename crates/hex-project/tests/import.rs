use hex_project::{Command, Created, Editor, MAX_TRACKS, Project};
use std::path::PathBuf;

fn editor(capacity: usize) -> Editor {
    Editor::new(Project::new(48_000).expect("valid project rate"), capacity)
        .expect("valid history capacity")
}

fn import_command(name: &str, path: &str, rate: u32, frames: u64) -> Command {
    Command::ImportAudioTrack {
        name: name.into(),
        path: PathBuf::from(path),
        sample_rate: rate,
        frames,
    }
}

#[test]
fn import_creates_complete_audio_track_as_one_undo_step() {
    let mut editor = editor(8);
    let ids = match editor
        .execute(import_command(
            "Imported Case",
            "Samples/Drum.KICK.WAV",
            48_000,
            12_345,
        ))
        .expect("import command")
    {
        Created::Imported { track, asset, clip } => (track, asset, clip),
        result => panic!("unexpected creation result: {result:?}"),
    };
    let imported = editor.project().clone();
    let (track_id, asset_id, clip_id) = ids;
    assert_eq!(imported.tracks().len(), 1);
    assert_eq!(imported.assets().count(), 1);
    let track = imported.track(track_id).expect("imported track");
    assert_eq!(track.name(), "Imported Case");
    assert_eq!(track.clips().len(), 1);
    assert_eq!(track.clips()[0].id(), clip_id);
    let asset = imported.asset(asset_id).expect("imported asset");
    assert_eq!(asset.path(), PathBuf::from("Samples/Drum.KICK.WAV"));
    assert_eq!(asset.frames(), 12_345);
    assert_eq!(asset.source_sample_rate(), 48_000);
    assert_eq!(asset.source_frames(), 12_345);
    assert_eq!(asset.playback_frames(), 12_345);
    assert_eq!(asset.conversion_version(), 0);
    let clip = imported.clip(clip_id).expect("imported clip");
    assert_eq!(clip.asset(), asset_id);
    assert_eq!(clip.start(), 0);
    assert_eq!(clip.duration(), 12_345);
    assert_eq!(clip.source_offset(), 0);
    assert_eq!(clip.source_length(), 12_345);
    assert!(!clip.looping());

    assert!(editor.undo());
    assert!(editor.project().tracks().is_empty());
    assert_eq!(editor.project().assets().count(), 0);
    assert!(
        !editor.can_undo(),
        "the import must be exactly one undo step"
    );
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert_eq!(editor.project(), &imported);
    assert_eq!(
        editor
            .project()
            .clip(clip_id)
            .expect("restored clip")
            .asset(),
        asset_id
    );
}

#[test]
fn prepared_import_adds_audio_to_an_existing_midi_track_in_one_history_step() {
    let mut editor = editor(8);
    let track = match editor
        .execute(Command::AddTrack {
            name: "hybrid".into(),
        })
        .expect("hybrid track")
    {
        Created::Track(track) => track,
        other => panic!("unexpected track: {other:?}"),
    };
    let midi_clip = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        })
        .expect("midi clip")
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected clip: {other:?}"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi_clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("midi note");
    let before = editor.project().clone();

    let result = editor
        .execute(Command::ImportPreparedAudioClip {
            track,
            start: 17,
            path: PathBuf::from("Samples/Audio.wav"),
            source_sample_rate: 48_000,
            source_frames: 16,
            playback_frames: 16,
            conversion_version: 0,
        })
        .expect("import on existing hybrid track");
    let Created::Imported {
        track: imported_track,
        asset,
        clip,
    } = result
    else {
        panic!("unexpected import result: {result:?}");
    };
    assert_eq!(imported_track, track);
    let imported = editor.project().clone();
    let lane = imported.track(track).expect("hybrid track");
    assert_eq!(lane.clips().len(), 1);
    assert_eq!(lane.clips()[0].id(), clip);
    assert_eq!(lane.clips()[0].asset(), asset);
    assert_eq!(lane.clips()[0].start(), 17);
    assert_eq!(lane.midi_clips().len(), 1);
    assert_eq!(lane.midi_clips()[0].id(), midi_clip);
    assert_eq!(lane.midi_clips()[0].notes().len(), 1);

    assert!(editor.undo());
    assert_eq!(editor.project().tracks(), before.tracks());
    assert_eq!(editor.project().assets().count(), 0);
    assert!(editor.project().snapshot().next_id > before.snapshot().next_id);
    assert!(editor.redo());
    assert_eq!(editor.project(), &imported);
}

#[test]
fn prepared_import_keeps_source_and_project_frame_domains_in_one_history_step() {
    let mut editor = editor(8);
    let created = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "converted".into(),
            path: "Media/Source-44100.wav".into(),
            source_sample_rate: 44_100,
            source_frames: 44_100,
            playback_frames: 48_000,
            conversion_version: 1,
        })
        .expect("valid conversion metadata");
    let Created::Imported { asset, clip, .. } = created else {
        panic!("unexpected import result: {created:?}");
    };
    let imported = editor.project().clone();
    let source = editor.project().asset(asset).expect("asset");
    assert_eq!(source.source_sample_rate(), 44_100);
    assert_eq!(source.source_frames(), 44_100);
    assert_eq!(source.playback_frames(), 48_000);
    assert_eq!(
        editor.project().clip(clip).expect("clip").duration(),
        48_000
    );
    assert_eq!(
        editor.project().clip(clip).expect("clip").source_length(),
        48_000
    );
    assert!(editor.undo());
    assert!(editor.project().assets().next().is_none());
    assert!(editor.redo());
    assert_eq!(editor.project(), &imported);
}

#[test]
fn prepared_import_rejects_bad_counts_versions_and_same_rate_conversion_atomically() {
    let mut editor = editor(8);
    for (source_rate, source_frames, playback_frames, version) in [
        (44_100, 44_100, 47_999, 1),
        (44_100, 44_100, 48_000, 0),
        (48_000, 100, 100, 1),
        (44_100, 44_100, 48_000, 99),
        (192_001, 1, 1, 0),
    ] {
        let before = editor.project().clone();
        assert!(
            editor
                .execute(Command::ImportPreparedAudioTrack {
                    name: "invalid".into(),
                    path: "source.wav".into(),
                    source_sample_rate: source_rate,
                    source_frames,
                    playback_frames,
                    conversion_version: version,
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert!(!editor.can_undo());
    }
}

#[test]
fn invalid_imports_are_atomic_and_keep_redo_branch() {
    let mut editor = editor(8);
    let track = match editor
        .execute(Command::AddTrack {
            name: "existing".into(),
        })
        .expect("add initial track")
    {
        Created::Track(id) => id,
        result => panic!("unexpected creation result: {result:?}"),
    };
    editor
        .execute(Command::Gain { track, gain: 0.5 })
        .expect("change gain");
    assert!(editor.undo());
    let before = editor.project().clone();

    for command in [
        import_command("", "Samples/Source.wav", 48_000, 10),
        import_command("valid", "", 48_000, 10),
        import_command("valid", "Samples/Source.wav", 44_100, 10),
        import_command("valid", "Samples/Source.wav", 48_000, 0),
    ] {
        assert!(editor.execute(command).is_err());
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
    }

    assert!(editor.redo());
    assert_eq!(editor.project().track(track).expect("track").gain(), 0.5);
}

#[test]
fn failed_capacity_check_is_atomic_and_does_not_erase_redo() {
    let mut editor = editor(1);
    let mut last_track = None;
    for index in 0..MAX_TRACKS {
        let result = editor
            .execute(Command::AddTrack {
                name: format!("track {index}"),
            })
            .expect("fill track capacity");
        if let Created::Track(id) = result {
            last_track = Some(id);
        }
    }
    let track = last_track.expect("at least one track");
    editor
        .execute(Command::Gain { track, gain: 0.5 })
        .expect("change gain");
    assert!(editor.undo());
    let before = editor.project().clone();

    assert!(
        editor
            .execute(import_command(
                "overflow",
                "Samples/Overflow.wav",
                48_000,
                100,
            ))
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
}

#[test]
fn imported_ids_are_not_reused_after_undo_branch() {
    let mut editor = editor(8);
    let first = match editor
        .execute(import_command("first", "Media/First.Wav", 48_000, 16))
        .expect("first import")
    {
        Created::Imported { track, asset, clip } => (track, asset, clip),
        result => panic!("unexpected creation result: {result:?}"),
    };
    assert!(editor.undo());
    let second = match editor
        .execute(import_command("second", "Media/Second.Wav", 48_000, 24))
        .expect("branched import")
    {
        Created::Imported { track, asset, clip } => (track, asset, clip),
        result => panic!("unexpected creation result: {result:?}"),
    };
    assert!(second.0.value() > first.2.value());
    assert!(second.1.value() > first.2.value());
    assert!(second.2.value() > first.2.value());
    assert!(!editor.can_redo());
}
