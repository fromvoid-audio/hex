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
            start: 24_000,
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
    assert_eq!(lane.clips()[0].start(), 24_000);
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
fn prepared_track_drop_inserts_at_stable_anchor_with_clip_start_and_one_history_step() {
    let mut editor = editor(8);
    let mut tracks = Vec::new();
    for name in ["first", "middle", "last"] {
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: name.into() })
            .expect("create track")
        else {
            panic!("expected track");
        };
        tracks.push(track);
    }
    let before = editor.project().clone();
    let result = editor
        .execute(Command::ImportPreparedAudioTrackAt {
            name: "inserted sample".into(),
            before_track: Some(tracks[1]),
            start: 12_345,
            path: PathBuf::from("samples/Drop.WAV"),
            source_sample_rate: 48_000,
            source_frames: 480,
            playback_frames: 480,
            conversion_version: 0,
        })
        .expect("insert track and import");
    let Created::Imported { track, asset, clip } = result else {
        panic!("expected atomic imported result");
    };
    let inserted = editor.project().clone();
    let order: Vec<_> = inserted.tracks().iter().map(|item| item.id()).collect();
    assert_eq!(order, [tracks[0], track, tracks[1], tracks[2]]);
    assert_eq!(
        inserted.track(track).expect("inserted track").clips()[0].id(),
        clip
    );
    assert_eq!(inserted.clip(clip).expect("imported clip").asset(), asset);
    assert_eq!(inserted.clip(clip).expect("imported clip").start(), 12_345);

    assert!(editor.undo());
    assert_eq!(editor.project().tracks(), before.tracks());
    assert_eq!(editor.project().assets().count(), before.assets().count());
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert_eq!(editor.project(), &inserted);
}

#[test]
fn prepared_track_drop_appends_and_invalid_anchor_or_overflow_is_atomic() {
    let mut editor = editor(8);
    let Created::Track(anchor) = editor
        .execute(Command::AddTrack {
            name: "anchor".into(),
        })
        .expect("anchor track")
    else {
        panic!("expected anchor");
    };
    let Created::Track(stale_anchor) = editor
        .execute(Command::AddTrack {
            name: "temporary".into(),
        })
        .expect("temporary track")
    else {
        panic!("expected temporary track");
    };
    editor
        .execute(Command::RemoveTrack(stale_anchor))
        .expect("remove temporary track");
    let before = editor.project().clone();
    let can_undo_before = editor.can_undo();
    let can_redo_before = editor.can_redo();
    let make = |before_track, start| Command::ImportPreparedAudioTrackAt {
        name: "sample".into(),
        before_track,
        start,
        path: PathBuf::from("source.wav"),
        source_sample_rate: 48_000,
        source_frames: 1,
        playback_frames: 1,
        conversion_version: 0,
    };
    assert!(editor.execute(make(Some(stale_anchor), 0)).is_err());
    assert!(editor.execute(make(None, u64::MAX)).is_err());
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.can_undo(), can_undo_before);
    assert_eq!(editor.can_redo(), can_redo_before);

    let Created::Imported { track, clip, .. } =
        editor.execute(make(None, 77)).expect("append sample track")
    else {
        panic!("expected imported track");
    };
    assert_eq!(
        editor.project().tracks().last().map(|item| item.id()),
        Some(track)
    );
    assert_eq!(editor.project().clip(clip).expect("clip").start(), 77);
    assert_eq!(editor.project().tracks()[0].id(), anchor);
}

#[test]
fn moving_audio_clip_to_new_hybrid_track_preserves_identity_and_is_one_undo_step() {
    let mut editor = editor(8);
    let source = match editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .expect("source")
    {
        Created::Track(track) => track,
        other => panic!("unexpected source: {other:?}"),
    };
    let anchor = match editor
        .execute(Command::AddTrack {
            name: "anchor".into(),
        })
        .expect("anchor")
    {
        Created::Track(track) => track,
        other => panic!("unexpected anchor: {other:?}"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: "Source/Pad.wav".into(),
            sample_rate: 48_000,
            frames: 256,
        })
        .expect("asset")
    {
        Created::Asset(asset) => asset,
        other => panic!("unexpected asset: {other:?}"),
    };
    let clip = match editor
        .execute(Command::PlaceClip {
            track: source,
            asset,
            start: 32,
            duration: 64,
            source_offset: 16,
            source_length: 96,
            looping: true,
        })
        .expect("clip")
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected clip: {other:?}"),
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 8,
            fade_out: 12,
        })
        .expect("clip fades");
    let original = editor.project().clone();
    let original_clip = original.clip(clip).expect("original clip").clone();

    let Created::Track(inserted) = editor
        .execute(Command::MoveClipToNewTrack {
            initial_gain: None,
            clip,
            name: "track 3".into(),
            before_track: Some(anchor),
            start: 80,
        })
        .expect("move to inserted track")
    else {
        panic!("inserted track expected")
    };
    let moved = editor.project().clone();
    assert_eq!(
        moved
            .tracks()
            .iter()
            .map(|track| track.id())
            .collect::<Vec<_>>(),
        [source, inserted, anchor]
    );
    assert!(
        moved
            .track(source)
            .expect("source remains")
            .clips()
            .is_empty()
    );
    let inserted_track = moved.track(inserted).expect("inserted hybrid track");
    assert!(inserted_track.midi_clips().is_empty());
    assert_eq!(inserted_track.clips()[0].id(), clip);
    assert_eq!(inserted_track.clips()[0].asset(), asset);
    assert_eq!(inserted_track.clips()[0].start(), 80);
    assert_eq!(inserted_track.clips()[0].source_offset(), 16);
    assert_eq!(inserted_track.clips()[0].fade(), original_clip.fade());
    assert_eq!(
        moved.asset(asset).expect("asset retained").path(),
        original.asset(asset).expect("original asset").path()
    );
    assert_eq!(
        Project::from_snapshot(moved.snapshot()).expect("snapshot round trip"),
        moved
    );
    assert!(editor.undo(), "insert+move is one history step");
    assert_eq!(editor.project().tracks(), original.tracks());
    assert_eq!(editor.project().asset(asset), original.asset(asset));
    assert!(editor.redo());
    assert_eq!(editor.project(), &moved);
}

#[test]
fn failed_audio_clip_insert_move_preserves_ids_order_and_redo() {
    let mut editor = editor(8);
    let track = match editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .expect("source")
    {
        Created::Track(track) => track,
        other => panic!("unexpected source: {other:?}"),
    };
    let missing_anchor = match editor
        .execute(Command::AddTrack {
            name: "anchor".into(),
        })
        .expect("anchor")
    {
        Created::Track(track) => track,
        other => panic!("unexpected anchor: {other:?}"),
    };
    editor
        .execute(Command::RemoveTrack(missing_anchor))
        .expect("remove anchor");
    let asset = match editor
        .execute(Command::AddAsset {
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 16,
        })
        .expect("asset")
    {
        Created::Asset(asset) => asset,
        other => panic!("unexpected asset: {other:?}"),
    };
    let clip = match editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 16,
            source_offset: 0,
            source_length: 16,
            looping: false,
        })
        .expect("clip")
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected clip: {other:?}"),
    };
    editor
        .execute(Command::AddTrack {
            name: "redo branch".into(),
        })
        .expect("redo branch");
    editor.undo();
    let before = editor.project().clone();
    let invalid = editor.execute(Command::MoveClipToNewTrack {
        initial_gain: None,
        clip,
        name: "new".into(),
        before_track: Some(missing_anchor),
        start: 1,
    });
    assert!(invalid.is_err());
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
}

#[test]
fn overwrite_audio_move_splits_victim_with_canonical_phase_fades_and_one_step_history() {
    let mut editor = editor(12);
    let target = match editor
        .execute(Command::AddTrack {
            name: "target".into(),
        })
        .expect("target track")
    {
        Created::Track(track) => track,
        other => panic!("unexpected target: {other:?}"),
    };
    let source = match editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .expect("source track")
    {
        Created::Track(track) => track,
        other => panic!("unexpected source: {other:?}"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: "Audio/overwrite.wav".into(),
            sample_rate: 48_000,
            frames: 256,
        })
        .expect("asset")
    {
        Created::Asset(asset) => asset,
        other => panic!("unexpected asset: {other:?}"),
    };
    let victim = match editor
        .execute(Command::PlaceClip {
            track: target,
            asset,
            start: 10,
            duration: 100,
            source_offset: 0,
            source_length: 20,
            looping: true,
        })
        .expect("looping victim")
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected victim: {other:?}"),
    };
    editor
        .execute(Command::SetClipFades {
            clip: victim,
            fade_in: 20,
            fade_out: 30,
        })
        .expect("victim fades");
    let incoming = match editor
        .execute(Command::PlaceClip {
            track: source,
            asset,
            start: 40,
            duration: 20,
            source_offset: 100,
            source_length: 20,
            looping: false,
        })
        .expect("incoming clip")
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected incoming: {other:?}"),
    };
    let before = editor.project().clone();

    editor
        .execute(Command::MoveClipWithOverwrite {
            clip: incoming,
            track: target,
            start: 40,
        })
        .expect("overwrite destination");
    let result = editor.project();
    let target_clips = result.track(target).expect("target").clips();
    let left = target_clips
        .iter()
        .find(|clip| clip.id() == victim)
        .expect("left victim");
    assert_eq!((left.start(), left.duration()), (10, 30));
    assert_eq!(left.source_phase(), 0);
    assert_eq!(left.fade().span(), 100);
    assert_eq!(left.fade().offset(), 0);
    let right = target_clips
        .iter()
        .find(|clip| clip.id() != victim && clip.id() != incoming)
        .expect("right victim fragment");
    assert_eq!((right.start(), right.duration()), (60, 50));
    assert_eq!(right.asset(), asset);
    assert_eq!(right.source_phase(), 10);
    assert_eq!(right.fade().span(), 100);
    assert_eq!(right.fade().offset(), 50);
    assert!(target_clips.iter().any(|clip| clip.id() == incoming));
    assert!(result.track(source).expect("source").clips().is_empty());
    let committed = result.clone();
    assert_eq!(
        Project::from_snapshot(committed.snapshot()).expect("overwrite roundtrip"),
        committed
    );
    assert!(editor.undo());
    let undone = editor.project().snapshot();
    let mut expected = before.snapshot();
    // Undo intentionally keeps the allocator high-water mark so stale IDs
    // cannot be reused after an overwrite creates split fragments.
    expected.next_id = undone.next_id;
    assert_eq!(undone, expected);
    assert!(editor.can_redo(), "overwrite is one history step");
    assert!(editor.redo());
    assert_eq!(editor.project(), &committed);
    let same_position = editor.project().snapshot();
    let can_undo = editor.can_undo();
    editor
        .execute(Command::MoveClipWithOverwrite {
            clip: incoming,
            track: target,
            start: 40,
        })
        .expect("same-position drop");
    assert_eq!(editor.project().snapshot(), same_position);
    assert_eq!(
        editor.can_undo(),
        can_undo,
        "same-position drop adds no history"
    );
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
    assert!(
        editor
            .execute(Command::ImportPreparedAudioTrackAt {
                name: "overflow".into(),
                before_track: Some(track),
                start: 0,
                path: "Samples/Overflow.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 100,
                playback_frames: 100,
                conversion_version: 0,
            })
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

#[test]
fn beats_import_is_visible_before_one_prepare_and_refusal_preserves_redo_and_ids() {
    use hex_project::{AudioImportWarp, WarpMode};
    let mut editor = editor(8);
    editor
        .execute(Command::AddTrack {
            name: "existing".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let commands = [24_000, 12_000].map(|frames| Command::ImportPreparedAudioTrackAt {
        name: "sample".into(),
        before_track: None,
        start: 0,
        path: PathBuf::from(format!("{frames}.wav")),
        source_sample_rate: 48_000,
        source_frames: frames,
        playback_frames: frames,
        conversion_version: 0,
    });
    let mut calls = 0;
    let result = editor.execute_audio_imports_with_warp_prepared(
        commands.to_vec(),
        AudioImportWarp::Beats,
        |candidate, results| {
            calls += 1;
            assert_eq!(results.len(), 2);
            for (index, track) in candidate.tracks().iter().enumerate() {
                let clip = &track.clips()[0];
                let domain = &clip.audio_edit().unwrap().domain;
                assert!(domain.warp_enabled);
                let settings = domain.warp.as_ref().unwrap();
                assert!(matches!(settings.mode, WarpMode::Beats { .. }));
                assert_eq!(
                    settings.markers[1].source_frame,
                    if index == 0 { 24_000 } else { 12_000 }
                );
            }
            Err::<(), _>(hex_project::Invalid("injected preparation refusal"))
        },
    );
    assert!(result.is_err());
    assert_eq!(calls, 1);
    assert_eq!(editor.project(), &before);
    assert!(!editor.can_undo());
    assert!(editor.can_redo());
    // Existing zero-warp entry point remains an explicit Off default.
    editor
        .execute_audio_imports_prepared(commands.to_vec(), |candidate, _| {
            assert!(
                candidate
                    .tracks()
                    .iter()
                    .all(|track| track.clips()[0].audio_edit().is_none())
            );
            Ok::<_, hex_project::Invalid>(())
        })
        .unwrap();
}

#[test]
fn new_device_imports_record_sinc_v2_or_identity_conversion() {
    let mut editor = editor(4);
    for (rate, expected) in [
        (44_100, hex_project::SINC_CONVERSION_V2),
        (48_000, hex_project::IDENTITY_CONVERSION),
    ] {
        let Created::Asset(asset) = editor
            .execute(Command::ImportDeviceAsset {
                path: PathBuf::from(format!("Media/device-{rate}.wav")),
                sample_rate: rate,
                frames: 4_410,
            })
            .expect("device asset")
        else {
            panic!("asset")
        };
        let asset = editor.project().asset(asset).expect("asset");
        assert_eq!(asset.conversion_version(), expected);
    }
}
