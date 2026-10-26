//! Shared content survives local marker edits; note edits detach retained history.
use crate::{BeatTime, Command, Created, Editor, Project};
use std::sync::Arc;

fn check_window_history(console: bool) {
    let beat = |n| BeatTime::from_quarters(n).unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let command = if console {
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        Command::CreateSessionMidiClip {
            track,
            scene,
            name: "clip".into(),
            source_length: beat(4),
        }
    } else {
        Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beat(4),
        }
    };
    let clip = match editor.execute(command).unwrap() {
        Created::Clip(clip) | Created::SessionClip(clip) => clip,
        other => panic!("unexpected clip result: {other:?}"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(2),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let content = Arc::clone(&editor.project().midi_source(clip).unwrap().content);
    let command = if console {
        Command::SetSessionMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: beat(1),
            loop_start: beat(1),
            loop_end: beat(4),
            looping: true,
        }
    } else {
        Command::SetMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: beat(1),
            loop_start: beat(1),
            loop_end: beat(4),
            looping: true,
            length: beat(4),
        }
    };
    editor.execute(command).unwrap();
    let source = editor.project().midi_source(clip).unwrap();
    assert!(
        Arc::ptr_eq(&content, &source.content),
        "local markers retain the actual note backing"
    );
    assert_eq!(source.source_start(), beat(1));
    assert!(source.looping());
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(3),
            duration: beat(1),
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let source = editor.project().midi_source(clip).unwrap();
    assert!(!Arc::ptr_eq(&content, &source.content));
    assert_eq!(source.notes().len(), 2);
    assert_eq!(content.notes().len(), 1, "retained history is immutable");
    assert!(editor.undo());
    assert!(Arc::ptr_eq(
        &content,
        &editor.project().midi_source(clip).unwrap().content
    ));
    assert!(editor.undo());
    assert_eq!(
        editor.project().midi_source(clip).unwrap().source_start(),
        BeatTime::ZERO
    );
    assert!(editor.redo());
    assert!(editor.redo());
    assert_eq!(editor.project().midi_source(clip).unwrap().notes().len(), 2);
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
}

#[test]
fn arrangement_markers_retain_content_and_note_edits_preserve_history() {
    check_window_history(false);
}

#[test]
fn console_markers_retain_content_and_note_edits_preserve_history() {
    check_window_history(true);
}
