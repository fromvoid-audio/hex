use hex_project::{Command, Created, Editor, Project, SessionCellContent};

fn fixture() -> (
    Editor,
    hex_project::TrackId,
    hex_project::TrackId,
    hex_project::SceneId,
    hex_project::AssetId,
) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let audio = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .expect("audio track")
    {
        Created::Track(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    let instrument = match editor
        .execute(Command::AddMidiTrack {
            name: "instrument".into(),
        })
        .expect("instrument")
    {
        Created::Track(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .expect("scene")
    {
        Created::Scene(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: "fixture.wav".into(),
            sample_rate: 48_000,
            frames: 32,
        })
        .expect("asset")
    {
        Created::Asset(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    (editor, audio, instrument, scene, asset)
}

#[test]
fn session_audio_is_available_on_hybrid_tracks_and_protects_referenced_assets() {
    let (mut editor, audio, instrument, scene, asset) = fixture();
    assert!(
        editor
            .execute(Command::CreateSessionAudioClip {
                track: audio,
                scene,
                name: "bad range".into(),
                asset,
                source_start: 8,
                source_end: 8
            })
            .is_err()
    );
    assert_eq!(editor.project().session_cells().len(), 0);
    let clip = match editor
        .execute(Command::CreateSessionAudioClip {
            track: instrument,
            scene,
            name: "region".into(),
            asset,
            source_start: 7,
            source_end: 29,
        })
        .expect("valid audio source")
    {
        Created::SessionClip(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    assert!(editor.execute(Command::RemoveAsset(asset)).is_err());
    assert_eq!(
        editor
            .project()
            .session_audio_clip(clip)
            .map(|item| (item.source_start(), item.source_end())),
        Some((7, 29))
    );
}

#[test]
fn arrangement_copy_and_scene_duplication_create_independent_audio_clip_ids() {
    let (mut editor, audio, _, scene, asset) = fixture();
    let arrangement = match editor
        .execute(Command::PlaceClip {
            track: audio,
            asset,
            start: 100,
            duration: 20,
            source_offset: 4,
            source_length: 20,
            looping: false,
        })
        .expect("arrangement placement")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    let copied = match editor
        .execute(Command::CopyArrangementAudioClipToSession {
            source: arrangement,
            scene,
        })
        .expect("copy to Session")
    {
        Created::SessionClip(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    let copied_clip = editor
        .project()
        .session_audio_clip(copied)
        .expect("copied source");
    assert_eq!(
        (copied_clip.source_start(), copied_clip.source_end()),
        (4, 24)
    );
    let duplicate_scene = match editor
        .execute(Command::DuplicateScene(scene))
        .expect("duplicate scene")
    {
        Created::Scene(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    let duplicated = match editor
        .project()
        .session_cell(audio, duplicate_scene)
        .expect("duplicated cell")
    {
        SessionCellContent::Audio(clip) => clip,
        SessionCellContent::Midi(_) | SessionCellContent::Stop => panic!("audio cell expected"),
    };
    assert_ne!(duplicated.id(), copied);
    assert_eq!(duplicated.asset(), asset);
    assert_eq!(
        (duplicated.source_start(), duplicated.source_end()),
        (4, 24)
    );
    let duplicated_id = duplicated.id();
    let third_scene = match editor
        .execute(Command::AddScene {
            name: "third".into(),
        })
        .expect("third scene")
    {
        Created::Scene(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    let cell_copy = match editor
        .execute(Command::DuplicateSessionAudioCell {
            track: audio,
            from_scene: scene,
            to_scene: third_scene,
        })
        .expect("duplicate Session audio cell")
    {
        Created::SessionClip(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    editor
        .execute(Command::SetSessionAudioClipSource {
            clip: cell_copy,
            asset,
            source_start: 10,
            source_end: 18,
        })
        .expect("edit copied source window");
    assert_eq!(
        editor
            .project()
            .session_audio_clip(copied)
            .map(|clip| (clip.source_start(), clip.source_end())),
        Some((4, 24))
    );
    assert_eq!(
        editor
            .project()
            .session_audio_clip(cell_copy)
            .map(|clip| (clip.source_start(), clip.source_end())),
        Some((10, 18))
    );
    assert!(editor.undo());
    assert_eq!(
        editor
            .project()
            .session_audio_clip(cell_copy)
            .map(|clip| (clip.source_start(), clip.source_end())),
        Some((4, 24))
    );
    assert!(editor.redo());
    assert!(
        editor
            .execute(Command::DuplicateSessionAudioCell {
                track: audio,
                from_scene: scene,
                to_scene: duplicate_scene
            })
            .is_err()
    );
    assert!(editor.undo());
    assert!(editor.redo());
    assert_eq!(
        editor
            .project()
            .clip(arrangement)
            .map(|clip| (clip.source_offset(), clip.source_length())),
        Some((4, 20))
    );
    assert_eq!(
        editor
            .project()
            .session_audio_clip(duplicated_id)
            .map(|clip| clip.asset()),
        Some(asset)
    );
}

#[test]
fn snapshots_reject_missing_audio_media_and_invalid_source_windows() {
    let (mut editor, audio, _, scene, asset) = fixture();
    editor
        .execute(Command::CreateSessionAudioClip {
            track: audio,
            scene,
            name: "region".into(),
            asset,
            source_start: 2,
            source_end: 9,
        })
        .expect("clip");
    let snapshot = editor.project().snapshot();
    let mut missing = snapshot.clone();
    let hex_project::SessionCellContentSnapshot::Audio(clip) =
        &mut missing.session.cells[0].content
    else {
        panic!("audio cell expected")
    };
    clip.asset = u64::MAX;
    assert!(Project::from_snapshot(missing).is_err());
    let mut outside = snapshot.clone();
    let hex_project::SessionCellContentSnapshot::Audio(clip) =
        &mut outside.session.cells[0].content
    else {
        panic!("audio cell expected")
    };
    clip.source_end = 33;
    assert!(Project::from_snapshot(outside).is_err());
    let mut moved_to_hybrid_track = snapshot.clone();
    moved_to_hybrid_track.session.cells[0].track = moved_to_hybrid_track.tracks[1].id;
    let moved = Project::from_snapshot(moved_to_hybrid_track).expect("audio on another track");
    assert_eq!(moved.session_cells()[0].track(), moved.tracks()[1].id());
    let mut colliding = snapshot;
    let hex_project::SessionCellContentSnapshot::Audio(clip) =
        &mut colliding.session.cells[0].content
    else {
        panic!("audio cell expected")
    };
    clip.id = colliding.tracks[0].id;
    assert!(Project::from_snapshot(colliding).is_err());
}

#[test]
fn invalid_audio_source_edits_preserve_project_history_and_redo_branch() {
    let (mut editor, audio, _, scene, asset) = fixture();
    let clip = match editor
        .execute(Command::CreateSessionAudioClip {
            track: audio,
            scene,
            name: "source".into(),
            asset,
            source_start: 2,
            source_end: 9,
        })
        .expect("clip")
    {
        Created::SessionClip(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    editor
        .execute(Command::SetSessionAudioClipSource {
            clip,
            asset,
            source_start: 5,
            source_end: 20,
        })
        .expect("valid source edit");
    assert!(editor.undo());
    let before_invalid_range = editor.project().clone();
    assert!(editor.can_redo());
    assert!(
        editor
            .execute(Command::SetSessionAudioClipSource {
                clip,
                asset,
                source_start: 9,
                source_end: 40,
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before_invalid_range);
    assert!(editor.can_redo());

    let removed_asset = match editor
        .execute(Command::AddAsset {
            path: "removed.wav".into(),
            sample_rate: 48_000,
            frames: 16,
        })
        .expect("extra asset")
    {
        Created::Asset(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    editor
        .execute(Command::RemoveAsset(removed_asset))
        .expect("remove unreferenced asset");
    editor
        .execute(Command::SetSessionAudioClipSource {
            clip,
            asset,
            source_start: 4,
            source_end: 12,
        })
        .expect("second valid source edit");
    assert!(editor.undo());
    let before_missing_asset = editor.project().clone();
    assert!(editor.can_redo());
    assert!(
        editor
            .execute(Command::SetSessionAudioClipSource {
                clip,
                asset: removed_asset,
                source_start: 0,
                source_end: 8,
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before_missing_asset);
    assert!(editor.can_redo());
}

#[test]
fn stale_audio_clip_ids_and_exhausted_project_ids_refuse_without_mutation() {
    let (mut editor, audio, _, scene, asset) = fixture();
    let clip = match editor
        .execute(Command::CreateSessionAudioClip {
            track: audio,
            scene,
            name: "stale source".into(),
            asset,
            source_start: 0,
            source_end: 8,
        })
        .expect("clip")
    {
        Created::SessionClip(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    editor
        .execute(Command::ClearSessionCell {
            track: audio,
            scene,
        })
        .expect("clear cell");
    let after_clear = editor.project().clone();
    assert!(
        editor
            .execute(Command::SetSessionAudioClipSource {
                clip,
                asset,
                source_start: 1,
                source_end: 7,
            })
            .is_err()
    );
    assert_eq!(editor.project(), &after_clear);

    let mut exhausted = editor.project().snapshot();
    exhausted.next_id = u64::MAX;
    let project = Project::from_snapshot(exhausted).expect("valid snapshot with exhausted ids");
    let mut exhausted = Editor::new(project, 16).expect("editor");
    let before = exhausted.project().clone();
    assert!(
        exhausted
            .execute(Command::CreateSessionAudioClip {
                track: audio,
                scene,
                name: "cannot allocate".into(),
                asset,
                source_start: 0,
                source_end: 8,
            })
            .is_err()
    );
    assert_eq!(exhausted.project(), &before);
}

#[test]
fn arrangement_and_session_audio_share_the_global_clip_limit() {
    let (mut editor, audio, _, scene, asset) = fixture();
    editor
        .execute(Command::PlaceClip {
            track: audio,
            asset,
            start: 0,
            duration: 1,
            source_offset: 0,
            source_length: 1,
            looping: false,
        })
        .expect("seed arrangement clip");
    let mut snapshot = editor.project().snapshot();
    let first_id = snapshot.next_id;
    let audio_clips = snapshot.tracks[0].audio_clips_mut().expect("audio track");
    let template = audio_clips[0].clone();
    audio_clips.resize(hex_project::MAX_CLIPS, template);
    for (offset, clip) in audio_clips.iter_mut().enumerate() {
        clip.id = first_id + offset as u64;
        clip.start = offset as u64;
    }
    snapshot.next_id = first_id + hex_project::MAX_CLIPS as u64;
    let project = Project::from_snapshot(snapshot).expect("valid max-size arrangement");
    let mut editor = Editor::new(project, 16).expect("editor");
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::CreateSessionAudioClip {
                track: audio,
                scene,
                name: "over limit".into(),
                asset,
                source_start: 0,
                source_end: 8,
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
}

#[test]
fn audio_session_cell_move_clear_scene_delete_and_track_delete_follow_history() {
    let (mut editor, audio, _, scene, asset) = fixture();
    let second_track = match editor
        .execute(Command::AddTrack {
            name: "second audio".into(),
        })
        .expect("second track")
    {
        Created::Track(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    let second_scene = match editor
        .execute(Command::AddScene {
            name: "second scene".into(),
        })
        .expect("second scene")
    {
        Created::Scene(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    let clip = match editor
        .execute(Command::CreateSessionAudioClip {
            track: audio,
            scene,
            name: "source".into(),
            asset,
            source_start: 1,
            source_end: 8,
        })
        .expect("clip")
    {
        Created::SessionClip(id) => id,
        other => panic!("unexpected: {other:?}"),
    };
    editor
        .execute(Command::MoveSessionCell {
            from_track: audio,
            from_scene: scene,
            to_track: second_track,
            to_scene: second_scene,
        })
        .expect("move audio cell");
    assert!(editor.project().session_cell(audio, scene).is_none());
    assert!(matches!(
        editor.project().session_cell(second_track, second_scene),
        Some(SessionCellContent::Audio(item)) if item.id() == clip
    ));
    assert!(editor.undo());
    assert!(matches!(
        editor.project().session_cell(audio, scene),
        Some(SessionCellContent::Audio(item)) if item.id() == clip
    ));
    assert!(editor.redo());
    editor
        .execute(Command::ClearSessionCell {
            track: second_track,
            scene: second_scene,
        })
        .expect("clear moved cell");
    assert!(
        editor
            .project()
            .session_cell(second_track, second_scene)
            .is_none()
    );
    assert!(editor.undo());
    assert!(editor.project().session_audio_clip(clip).is_some());
    editor
        .execute(Command::RemoveScene(second_scene))
        .expect("remove scene");
    assert!(
        editor
            .project()
            .session_cell(second_track, second_scene)
            .is_none()
    );
    assert!(editor.undo());
    assert!(editor.project().session_audio_clip(clip).is_some());
    editor
        .execute(Command::RemoveTrack(second_track))
        .expect("remove track");
    assert!(editor.project().track(second_track).is_none());
    assert!(editor.undo());
    assert!(editor.project().track(second_track).is_some());
}

#[test]
fn console_audio_cycles_preserve_retained_source_history_and_duplicate_independently() {
    use hex_project::SessionAudioLoop;
    let (mut editor, track, _, scene, asset) = fixture();
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "intro and loop".into(),
            asset,
            source_start: 3,
            source_end: 29,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let original = editor.project().clone();
    let cycle = SessionAudioLoop::new(7, 13).unwrap();
    editor
        .execute(Command::SetSessionAudioClipLoop {
            clip,
            loop_range: Some(cycle),
        })
        .unwrap();
    let looping = editor.project().clone();
    let item = looping.session_audio_clip(clip).unwrap();
    assert_eq!(
        (item.source_start(), item.source_end(), item.loop_range()),
        (3, 29, Some(cycle))
    );
    assert_eq!(Project::from_snapshot(looping.snapshot()).unwrap(), looping);
    for invalid in [
        SessionAudioLoop::new(0, 3).unwrap(),
        SessionAudioLoop::new(28, 30).unwrap(),
    ] {
        assert!(
            editor
                .execute(Command::SetSessionAudioClipLoop {
                    clip,
                    loop_range: Some(invalid)
                })
                .is_err()
        );
        assert_eq!(editor.project(), &looping);
    }
    assert!(SessionAudioLoop::new(7, 7).is_err());
    assert!(
        editor
            .execute(Command::SetSessionAudioClipSource {
                clip,
                asset,
                source_start: 13,
                source_end: 29
            })
            .is_err()
    );
    assert_eq!(editor.project(), &looping);
    assert!(editor.undo());
    assert_eq!(editor.project(), &original);
    assert!(editor.redo());
    assert_eq!(editor.project(), &looping);
    let Created::Scene(duplicate) = editor.execute(Command::DuplicateScene(scene)).unwrap() else {
        panic!("scene")
    };
    let SessionCellContent::Audio(copied) =
        editor.project().session_cell(track, duplicate).unwrap()
    else {
        panic!("audio")
    };
    assert_eq!(copied.loop_range(), Some(cycle));
    let copied_id = copied.id();
    editor
        .execute(Command::SetSessionAudioClipLoop {
            clip: copied_id,
            loop_range: None,
        })
        .unwrap();
    assert_eq!(
        editor
            .project()
            .session_audio_clip(clip)
            .unwrap()
            .loop_range(),
        Some(cycle)
    );
    assert_eq!(cycle.frame_at(3, u64::MAX), Some(7 + (u64::MAX - 10) % 6));
    assert_eq!(cycle.frame_at(13, 0), None);
}
