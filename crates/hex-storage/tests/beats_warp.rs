use hex_project::{
    BeatsPreserve, BeatsTail, Command, Created, Editor, Project, WarpMode, default_audio_warp,
};

#[test]
fn beats_clip_state_roundtrips_and_each_warp_toggle_has_one_undo_step() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "drums".into(),
            path: "unchanged-source.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 700 })
        .unwrap();
    for tail in [BeatsTail::Off, BeatsTail::Forward, BeatsTail::Alternating] {
        let before = editor.project().clone();
        let settings = default_audio_warp(
            editor.project().clip(clip).unwrap(),
            editor.project().tempo_map(),
            WarpMode::Beats {
                preserve: BeatsPreserve::Transients,
                tail,
                decay: 75,
            },
        )
        .unwrap();
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            })
            .unwrap();
        let warped = editor.project().clone();
        assert_eq!(
            hex_storage::decode(&hex_storage::encode(&warped).unwrap()).unwrap(),
            warped
        );
        assert_eq!(
            warped
                .clip(clip)
                .unwrap()
                .audio_edit()
                .unwrap()
                .domain
                .pitch_cents,
            700
        );
        assert!(editor.undo());
        assert_eq!(*editor.project(), before);
        assert!(editor.redo());
        assert_eq!(*editor.project(), warped);
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: None,
            })
            .unwrap();
        let disabled = editor.project().clone();
        assert!(
            !disabled
                .clip(clip)
                .unwrap()
                .audio_edit()
                .unwrap()
                .domain
                .warp_enabled
        );
        assert_eq!(
            hex_storage::decode(&hex_storage::encode(&disabled).unwrap()).unwrap(),
            disabled
        );
        assert!(editor.undo());
        assert_eq!(*editor.project(), warped);
        assert!(editor.redo());
        assert_eq!(*editor.project(), disabled);
    }
}
