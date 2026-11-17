use crate::{ClipEdit, Command, Created, Editor, Project, TrimEdge, WarpMode};

#[test]
fn enabling_warp_chooses_modes_per_source_and_keeps_one_history_step() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let [short, long] = [48_000, 20_000_000].map(|frames| {
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "audio".into(),
                path: format!("{frames}.wav").into(),
                sample_rate: 48_000,
                frames,
            })
            .unwrap()
        else {
            panic!("fixture import");
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        clip
    });
    let before = editor.project().snapshot();
    let enable = Command::EditClips {
        clips: vec![short, long],
        edit: ClipEdit::WarpEnabled { enabled: true },
    };
    editor.execute(enable.clone()).unwrap();
    let mode = |clip| {
        let item = editor.project().clip(clip).unwrap();
        let domain = &item.audio_edit().unwrap().domain;
        assert!(domain.warp_enabled);
        domain.warp.as_ref().unwrap().mode
    };
    assert!(matches!(mode(short), WarpMode::Beats { .. }));
    assert!(matches!(mode(long), WarpMode::Tones { .. }));
    let long_clip = editor.project().clip(long).unwrap();
    assert_eq!(long_clip.source_length(), 20_000_000);
    assert_eq!(long_clip.duration(), 20_000_000);
    let after = editor.project().snapshot();
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);

    let authored = WarpMode::Texture {
        grain_ms: 32,
        fluctuation: 35,
        seed: 42,
    };
    editor
        .execute(Command::EditClips {
            clips: vec![long],
            edit: ClipEdit::WarpMode { mode: authored },
        })
        .unwrap();
    assert_eq!(
        editor
            .project()
            .audio_warp_settings_for_enable(long)
            .unwrap()
            .mode,
        authored
    );

    assert!(editor.undo());
    assert!(editor.undo());
    editor
        .execute(Command::SetClipFades {
            clip: long,
            fade_in: 64,
            fade_out: 0,
        })
        .unwrap();
    let faded = editor.project().snapshot();
    assert!(editor.execute(enable).is_err());
    assert_eq!(editor.project().snapshot(), faded);
}

#[test]
fn long_streamable_warp_stretch_keeps_preview_command_and_history_aligned() {
    const SOURCE: u64 = 20_000_000;
    const END: u64 = 24_000_000;
    for mode in [
        WarpMode::RePitch,
        WarpMode::Tones { grain_ms: 32 },
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "long".into(),
                path: "long.wav".into(),
                sample_rate: 48_000,
                frames: SOURCE,
            })
            .unwrap()
        else {
            panic!("fixture import");
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        let project = editor.project();
        let settings =
            crate::default_audio_warp(project.clip(clip).unwrap(), project.tempo_map(), mode)
                .unwrap();
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            })
            .unwrap();
        let project = editor.project();
        let before = project.snapshot();
        let original = project.clip(clip).unwrap();
        let preview = crate::preview_audio_clip_stretch(
            original,
            TrimEdge::Right,
            END,
            project.tempo_map(),
            SOURCE,
        )
        .unwrap();
        assert_eq!(preview.duration(), END);
        assert_eq!(preview.asset(), original.asset());
        assert_eq!(preview.source_length(), SOURCE);
        assert_eq!(
            preview
                .audio_edit()
                .unwrap()
                .domain
                .warp
                .as_ref()
                .unwrap()
                .mode,
            mode
        );
        editor
            .execute(Command::StretchAudioClipEdge {
                clip,
                edge: TrimEdge::Right,
                frame: END,
            })
            .unwrap();
        assert_eq!(editor.project().clip(clip), Some(&preview));
        let after = editor.project().snapshot();
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}
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
