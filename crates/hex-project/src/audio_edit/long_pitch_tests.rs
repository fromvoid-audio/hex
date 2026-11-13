use crate::{Command, Created, Editor, Project};

#[test]
fn full_long_pitch_round_trips_history_and_reset_without_admitting_processed_shapes() {
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
        panic!("audio fixture")
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let plain = editor.project().snapshot();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: -700 })
        .unwrap();
    let pitched = editor.project().snapshot();
    assert_eq!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .pitch_cents,
        -700
    );
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), plain);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), pitched);
    let project = editor.project();
    let retained = project.clip(clip).unwrap().retained_snapshot();
    assert!(
        project
            .restore_retained_audio_root(
                1,
                retained,
                std::sync::Arc::new(project.tempo_map().clone())
            )
            .is_err()
    );
    for command in [
        Command::SetClipFades {
            clip,
            fade_in: 100,
            fade_out: 0,
        },
        Command::SetClipLooping {
            clip,
            looping: true,
        },
    ] {
        assert!(editor.execute(command).is_err());
        assert_eq!(editor.project().snapshot(), pitched);
    }
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 700 })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 0 })
        .unwrap();
    assert_eq!(editor.project().snapshot(), plain);
    assert!(editor.undo());
    assert_eq!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .pitch_cents,
        700
    );
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), plain);
}

#[test]
fn long_pitch_refuses_a_partial_asset_even_when_the_clip_region_is_complete() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "long".into(),
            path: "long.wav".into(),
            sample_rate: 48_000,
            frames: 20_000_000,
        })
        .unwrap()
    else {
        panic!("audio fixture")
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: crate::TrimEdge::Right,
            frame: 19_000_000,
        })
        .unwrap();
    let trimmed = editor.project().clip(clip).unwrap();
    assert_eq!(
        (
            trimmed.source_offset(),
            trimmed.source_length(),
            trimmed.duration()
        ),
        (0, 19_000_000, 19_000_000)
    );
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 700 })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
}
