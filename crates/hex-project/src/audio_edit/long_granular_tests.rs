use crate::{Command, Created, Editor, Project, WarpMode};
#[test]
fn full_long_granular_roots_preserve_history_and_refuse_resident_shapes() {
    for mode in [
        WarpMode::Tones { grain_ms: 32 },
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "long".into(),
                path: "long.wav".into(),
                sample_rate: 48_000,
                frames: 20_000_000,
            })
            .unwrap()
        else {
            panic!("fixture import")
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        let before = editor.project().snapshot();
        let warp = crate::default_audio_warp(
            editor.project().clip(clip).unwrap(),
            editor.project().tempo_map(),
            mode,
        )
        .unwrap();
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(warp),
            })
            .unwrap();
        let after = editor.project().snapshot();
        assert_eq!(
            Project::from_snapshot(after.clone()).unwrap().snapshot(),
            after
        );
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
        assert!(
            editor
                .execute(Command::SetClipFades {
                    clip,
                    fade_in: 8,
                    fade_out: 8
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), after);
        let mut unsupported = editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .warp
            .clone()
            .unwrap();
        unsupported.mode = WarpMode::Complex;
        assert!(
            editor
                .execute(Command::SetAudioClipWarp {
                    clip,
                    settings: Some(unsupported)
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), after);
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 300 })
            .unwrap();
        let cropped = editor.execute(Command::TrimClipEdge {
            clip,
            edge: crate::TrimEdge::Left,
            frame: 1000,
        });
        assert!(cropped.is_ok());
        let project = editor.project();
        assert!(
            project
                .restore_retained_audio_root(
                    1,
                    project.clip(clip).unwrap().retained_snapshot(),
                    std::sync::Arc::new(project.tempo_map().clone())
                )
                .is_err(),
            "resident processing custody remains bounded"
        );
    }
}
