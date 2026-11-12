use super::*;
use hex_project::{BeatTime, Command, Created, Editor};

/// Shared model-only fixture for default and support-tier clip-selection tests.
pub(super) fn fixture() -> (Project, Vec<ClipId>, ClipId) {
    let mut setup = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let Created::Imported {
        track,
        clip: imported,
        ..
    } = setup
        .execute(Command::ImportAudioTrack {
            name: "audio source".into(),
            path: "missing.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .expect("import")
    else {
        panic!("import result")
    };
    let Created::Track(target) = setup
        .execute(Command::AddTrack {
            name: "target".into(),
        })
        .expect("target track")
    else {
        panic!("track result")
    };
    let project = setup.project().clone();
    let asset = project.clip(imported).expect("imported clip").asset();
    let mut editor = Editor::new(project, 32).expect("selection fixture editor");
    editor
        .execute(Command::RemoveClip(imported))
        .expect("remove fixture clip");
    let mut clips = Vec::new();
    for index in 0..5 {
        let start = index * 36_000;
        let command = if index % 2 == 0 {
            Command::PlaceClip {
                track,
                asset,
                start,
                duration: 24_000,
                source_offset: 0,
                source_length: 24_000,
                looping: false,
            }
        } else {
            Command::PlaceMidiClip {
                track,
                start: editor
                    .project()
                    .tempo_map()
                    .frame_to_beat(start)
                    .expect("frame beat"),
                length: BeatTime::ONE,
            }
        };
        let Created::Clip(id) = editor.execute(command).expect("place fixture clip") else {
            panic!("clip")
        };
        clips.push(id);
    }
    let Created::Clip(other) = editor
        .execute(Command::PlaceClip {
            track: target,
            asset,
            start: 0,
            duration: 24_000,
            source_offset: 0,
            source_length: 24_000,
            looping: false,
        })
        .expect("place target clip")
    else {
        panic!("other clip")
    };
    (editor.project().clone(), clips, other)
}
