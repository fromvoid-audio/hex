use hex_project::{ClipFlavor, Command, Created, Editor, ElementColor, Project};
use hex_storage::{decode, encode};

#[test]
fn automatic_colors_survive_current_document_redo_and_repeated_decode_without_reselection() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    for i in 0..18 {
        let Created::Imported { track, clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: format!("audio {i}"),
                path: format!("not-read-{i}.wav").into(),
                sample_rate: 48_000,
                frames: 480,
            })
            .unwrap()
        else {
            panic!("track import")
        };
        assert_ne!(
            editor.project().track(track).unwrap().color(),
            ElementColor::Gray
        );
        let snapshot = editor.project().snapshot();
        assert!(editor.undo());
        assert!(editor.project().track(track).is_none());
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), snapshot);
        editor
            .execute(Command::SetClipFlavor {
                clip,
                flavor: ClipFlavor::Accent,
            })
            .unwrap();
    }
    let colors: Vec<_> = editor
        .project()
        .tracks()
        .iter()
        .map(|t| (t.id(), t.color()))
        .collect();
    let bytes = encode(editor.project()).unwrap();
    for _ in 0..4 {
        let loaded = decode(&bytes).unwrap();
        assert_eq!(&loaded, editor.project());
        assert_eq!(
            loaded
                .tracks()
                .iter()
                .map(|t| (t.id(), t.color()))
                .collect::<Vec<_>>(),
            colors
        );
        assert!(
            loaded
                .tracks()
                .iter()
                .flat_map(|t| t.clips())
                .all(|c| loaded.clip_flavor(c.id()) == Some(ClipFlavor::Accent))
        );
        assert_eq!(encode(&loaded).unwrap(), bytes);
    }
    // Explicitly authored Gray is still a valid current-format value, not reset.
    let track = colors[0].0;
    editor
        .execute(Command::SetTrackColor {
            track,
            color: ElementColor::Gray,
        })
        .unwrap();
    let loaded = decode(&encode(editor.project()).unwrap()).unwrap();
    assert_eq!(loaded.track(track).unwrap().color(), ElementColor::Gray);
}
