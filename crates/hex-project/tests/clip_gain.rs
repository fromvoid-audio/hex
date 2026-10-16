use hex_project::{ClipGain, ClipId, Command, Created, Editor, Project};

fn fixture() -> (Editor, ClipId, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "gain.wav".into(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Clip(clip) = editor
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
    let Created::Scene(scene) = editor
        .execute(Command::AddScene { name: "one".into() })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(session) = editor
        .execute(Command::CopyArrangementAudioClipToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("session")
    };
    (editor, clip, session)
}

#[test]
fn gain_range_neutral_and_atomic_history_cover_both_audio_domains() {
    assert_eq!(ClipGain::default(), ClipGain::UNITY);
    assert_eq!(ClipGain::UNITY.linear().to_bits(), 1.0_f32.to_bits());
    for value in [i32::MIN, -60_001, 24_001, i32::MAX] {
        assert!(ClipGain::new(value).is_err());
    }
    for value in [-60_000, 24_000] {
        assert!(ClipGain::new(value).unwrap().linear().is_finite());
    }
    let (mut editor, clip, session) = fixture();
    for id in [clip, session] {
        let before = editor.project().clone();
        for value in [-60_001, 24_001] {
            assert!(
                editor
                    .execute(Command::SetAudioClipGain {
                        clip: id,
                        millidb: value
                    })
                    .is_err()
            );
            assert_eq!(editor.project(), &before);
        }
        editor
            .execute(Command::SetAudioClipGain {
                clip: id,
                millidb: -6_000,
            })
            .unwrap();
        let changed = editor.project().clone();
        assert!(editor.undo());
        assert_eq!(editor.project(), &before);
        assert!(editor.redo());
        assert_eq!(editor.project(), &changed);
        assert_eq!(Project::from_snapshot(changed.snapshot()).unwrap(), changed);
    }
    assert_eq!(
        editor.project().clip(clip).unwrap().gain().millidb(),
        -6_000
    );
    assert_eq!(
        editor
            .project()
            .session_audio_clip(session)
            .unwrap()
            .gain()
            .millidb(),
        -6_000
    );
}

#[test]
fn gain_survives_split_trim_and_arrangement_to_session_copy() {
    let (mut editor, clip, _) = fixture();
    editor
        .execute(Command::SetAudioClipGain {
            clip,
            millidb: 3_000,
        })
        .unwrap();
    editor
        .execute(Command::SplitClip { clip, frame: 4 })
        .unwrap();
    let track = editor.project().tracks()[0].id();
    assert!(
        editor
            .project()
            .track(track)
            .unwrap()
            .clips()
            .iter()
            .all(|clip| clip.gain().millidb() == 3_000)
    );
    editor
        .execute(Command::TrimClip {
            clip,
            start: 1,
            duration: 3,
            source_offset: 1,
            source_length: 3,
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap().gain().millidb(), 3_000);
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "copy".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(copy) = editor
        .execute(Command::CopyArrangementAudioClipToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("session")
    };
    assert_eq!(
        editor
            .project()
            .session_audio_clip(copy)
            .unwrap()
            .gain()
            .millidb(),
        3_000
    );
    let Created::Scene(to_scene) = editor
        .execute(Command::AddScene {
            name: "duplicate".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(duplicate) = editor
        .execute(Command::DuplicateSessionAudioCell {
            track,
            from_scene: scene,
            to_scene,
        })
        .unwrap()
    else {
        panic!("duplicate")
    };
    assert_eq!(
        editor
            .project()
            .session_audio_clip(duplicate)
            .unwrap()
            .gain()
            .millidb(),
        3_000
    );
}

#[test]
fn duplicate_clip_reconstruction_preserves_gain_and_independent_history() {
    let (mut editor, clip, _) = fixture();
    editor
        .execute(Command::SetAudioClipGain {
            clip,
            millidb: -6_000,
        })
        .unwrap();
    let original = editor.project().clip(clip).unwrap().clone();
    let Created::Clip(duplicate) = editor.execute(Command::DuplicateClip(clip)).unwrap() else {
        panic!("duplicate")
    };
    assert_ne!(duplicate, clip);
    let copied = editor.project().clip(duplicate).unwrap();
    assert_eq!(copied.gain(), original.gain());
    assert_eq!(copied.start(), original.start() + original.duration());
    editor
        .execute(Command::SetAudioClipGain {
            clip: duplicate,
            millidb: 3_000,
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap().gain(), original.gain());
    assert!(editor.undo());
    assert_eq!(
        editor.project().clip(duplicate).unwrap().gain(),
        original.gain()
    );
    assert!(editor.undo());
    assert!(editor.project().clip(duplicate).is_none());
    assert!(editor.redo());
    assert_eq!(
        editor.project().clip(duplicate).unwrap().gain(),
        original.gain()
    );
}
