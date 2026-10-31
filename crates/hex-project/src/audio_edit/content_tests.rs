//! Local fades, loop markers and crops retain immutable authored content.
use crate::{Command, Created, Editor, Project, TrimEdge};
use std::sync::Arc;

#[test]
fn local_settings_retain_content_and_source_edits_detach_history() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "missing.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 100 })
        .unwrap();
    let content = Arc::clone(
        &editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .content,
    );
    for command in [
        Command::SetClipFades {
            clip,
            fade_in: 120,
            fade_out: 240,
        },
        Command::SetClipLooping {
            clip,
            looping: true,
        },
        Command::SetClipSourcePhase { clip, phase: 100 },
        Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: 72_000,
        },
    ] {
        editor.execute(command).unwrap();
        assert!(Arc::ptr_eq(
            &content,
            &editor
                .project()
                .clip(clip)
                .unwrap()
                .audio_edit()
                .unwrap()
                .domain
                .content
        ));
    }
    let local = editor.project().clip(clip).unwrap().clone();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 200 })
        .unwrap();
    let changed = editor.project().clip(clip).unwrap();
    assert!(!Arc::ptr_eq(
        &content,
        &changed.audio_edit().unwrap().domain.content
    ));
    assert_eq!(content.pitch_cents, 100);
    assert_eq!(changed.audio_edit().unwrap().domain.pitch_cents, 200);
    assert_eq!(
        (changed.start(), changed.duration(), changed.retained_fade()),
        (local.start(), local.duration(), local.retained_fade())
    );
    assert!(editor.undo());
    assert_eq!(editor.project().clip(clip).unwrap(), &local);
    assert!(editor.redo());
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
}

/// Section splicing freezes a reversed musical clip's processing tempo. Later
/// overwrite cuts crop that carrier in the frozen clock, so the directed read
/// window must take its length from the same crop, not the current tempo.
#[test]
fn overwrite_cuts_keep_frozen_reversed_read_length() {
    use crate::{BeatTime, ClipGroupPosition, ElementColor, Tempo};
    let mut editor = Editor::new(Project::new(44_100).unwrap(), 16).unwrap();
    let import = |editor: &mut Editor, name: &str| {
        let Created::Imported { clip, track, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: name.into(),
                path: format!("{name}.wav").into(),
                sample_rate: 44_100,
                frames: 40_965,
            })
            .unwrap()
        else {
            panic!("audio")
        };
        (clip, track)
    };
    let (clip, track) = import(&mut editor, "reversed");
    let (dropped, _) = import(&mut editor, "dropped");
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    editor.execute(Command::ReverseClips(vec![clip])).unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_quarters(1).unwrap(),
            tempo: Tempo::new(97_310_000).unwrap(),
        })
        .unwrap();
    let Created::ArrangementSection(section) = editor
        .execute(Command::AddArrangementSection {
            name: "a".into(),
            color: ElementColor::default(),
            start: BeatTime::from_quarters(0).unwrap(),
            end: BeatTime::from_quarters(3).unwrap(),
        })
        .unwrap()
    else {
        panic!("section")
    };
    editor
        .execute(Command::DuplicateArrangementSection(section))
        .unwrap();
    let frozen = editor.project().tracks()[0]
        .clips()
        .iter()
        .find(|c| {
            c.start() > 0
                && c.audio_edit()
                    .is_some_and(|e| e.processing_projection.is_some() && e.content_view.is_some())
        })
        .cloned()
        .expect("spliced frozen reversed clip");
    let before = editor.project().snapshot().tracks;
    for at in [76_434, 76_531] {
        let at = frozen.start() + at % frozen.duration();
        editor
            .execute(Command::SplitClip {
                clip: frozen.id(),
                frame: at,
            })
            .unwrap();
        assert_eq!(
            Project::from_snapshot(editor.project().snapshot()).unwrap(),
            *editor.project()
        );
        assert!(editor.undo());
        // A drop whose footprint ends inside the clip cuts its prefix.
        editor
            .execute(Command::MoveClipsWithOverwrite {
                clips: vec![dropped],
                anchor: dropped,
                track,
                position: ClipGroupPosition::Audio(at.saturating_sub(40_965)),
                copy: false,
            })
            .unwrap();
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot().tracks, before);
    }
}
