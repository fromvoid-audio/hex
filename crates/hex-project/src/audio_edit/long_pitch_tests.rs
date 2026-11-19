use crate::{Command, Created, Editor, Project};

#[test]
fn long_pitch_and_fades_can_be_edited_in_either_order() {
    for pitch_first in [false, true] {
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
        let pitch = Command::SetAudioClipPitch { clip, cents: 700 };
        let fades = Command::SetClipFades {
            clip,
            fade_in: 1000,
            fade_out: 2000,
        };
        let (first, second) = if pitch_first {
            (pitch, fades)
        } else {
            (fades, pitch)
        };
        editor.execute(first).unwrap();
        let before = editor.project().snapshot();
        editor.execute(second).unwrap();
        let after = editor.project().snapshot();
        let mut malformed = after.clone();
        let clips = match &mut malformed.tracks[0].content {
            crate::TrackContentSnapshot::Audio(clips) => clips,
            crate::TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            crate::TrackContentSnapshot::Instrument(_) => panic!("audio track"),
        };
        clips[0].fade = crate::ClipFade::new(20_000_000, 0, 1, 0).unwrap();
        assert!(Project::from_snapshot(malformed).is_err());
        let clip = editor.project().clip(clip).unwrap();
        assert_eq!(clip.retained_fade().fade_in(), 1000);
        assert_eq!(clip.retained_fade().fade_out(), 2000);
        assert_eq!(clip.audio_edit().unwrap().domain.pitch_cents, 700);
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
        let clip = editor.project().tracks()[0].clips()[0].id();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 0 })
            .unwrap();
        let reset = editor.project().clip(clip).unwrap();
        assert!(reset.audio_edit().is_none());
        assert_eq!(reset.fade().fade_in(), 1000);
        assert_eq!(reset.fade().fade_out(), 2000);
    }
}

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
    assert!(
        editor
            .execute(Command::SetClipLooping {
                clip,
                looping: true
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), pitched);
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

#[test]
fn long_repitch_refuses_a_partial_source() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
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
    let Created::Clip(clip) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 20_000_000,
            duration: 19_000_000,
            source_offset: 0,
            source_length: 19_000_000,
            looping: false,
        })
        .unwrap()
    else {
        panic!("clip fixture")
    };
    let warp = crate::default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        crate::WarpMode::RePitch,
    )
    .unwrap();
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(warp)
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
}
