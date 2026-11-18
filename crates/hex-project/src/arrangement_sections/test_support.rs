//! Shared complete-song fixture for absolute section placement regressions.
use super::*;
use crate::Editor;

pub(super) fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).unwrap()
}

fn marker(editor: &mut Editor, name: &str, start: u64, end: u64) -> SectionId {
    let Created::ArrangementSection(id) = editor
        .execute(Command::AddArrangementSection {
            name: name.into(),
            color: ElementColor::Blue,
            start: beat(start),
            end: beat(end),
        })
        .unwrap()
    else {
        panic!("section fixture");
    };
    id
}

pub(super) fn fixture() -> (Editor, SectionId) {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "owned.wav".into(),
            sample_rate: 8_000,
            frames: 16_000,
        })
        .unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("midi track");
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(0),
            length: beat(4),
        })
        .unwrap()
    else {
        panic!("midi clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 90,
        })
        .unwrap();
    editor
        .execute(Command::DuplicateAliasClips(vec![clip]))
        .unwrap();
    let Created::Clip(tail) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(12),
            length: beat(4),
        })
        .unwrap()
    else {
        panic!("unmarked tail");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: tail,
            start: beat(0),
            duration: beat(1),
            pitch: 72,
            channel: 0,
            velocity: 80,
        })
        .unwrap();
    let source = marker(&mut editor, "source", 0, 4);
    marker(&mut editor, "last", 4, 8);
    (Editor::new(editor.project().clone(), 8).unwrap(), source)
}
