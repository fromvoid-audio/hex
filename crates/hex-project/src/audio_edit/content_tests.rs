//! Local fades, loop markers and crops retain immutable authored content.
use crate::{Command, Created, Editor, Project, TrimEdge};
use std::sync::Arc;

#[test]
fn local_settings_retain_content_and_source_edits_detach_history() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, .. } = editor.execute(Command::ImportAudioTrack {
        name: "audio".into(), path: "missing.wav".into(), sample_rate: 48_000, frames: 96_000,
    }).unwrap() else { panic!("audio") };
    editor.execute(Command::SetAudioClipPitch { clip, cents: 100 }).unwrap();
    let content = Arc::clone(&editor.project().clip(clip).unwrap().audio_edit().unwrap().domain.content);
    for command in [
        Command::SetClipFades { clip, fade_in: 120, fade_out: 240 },
        Command::SetClipLooping { clip, looping: true },
        Command::SetClipSourcePhase { clip, phase: 100 },
        Command::TrimClipEdge { clip, edge: TrimEdge::Right, frame: 72_000 },
    ] {
        editor.execute(command).unwrap();
        assert!(Arc::ptr_eq(&content, &editor.project().clip(clip).unwrap().audio_edit().unwrap().domain.content));
    }
    let local = editor.project().clip(clip).unwrap().clone();
    editor.execute(Command::SetAudioClipPitch { clip, cents: 200 }).unwrap();
    let changed = editor.project().clip(clip).unwrap();
    assert!(!Arc::ptr_eq(&content, &changed.audio_edit().unwrap().domain.content));
    assert_eq!(content.pitch_cents, 100);
    assert_eq!(changed.audio_edit().unwrap().domain.pitch_cents, 200);
    assert_eq!((changed.start(), changed.duration(), changed.retained_fade()), (local.start(), local.duration(), local.retained_fade()));
    assert!(editor.undo());
    assert_eq!(editor.project().clip(clip).unwrap(), &local);
    assert!(editor.redo());
    assert_eq!(Project::from_snapshot(editor.project().snapshot()).unwrap(), *editor.project());
}
