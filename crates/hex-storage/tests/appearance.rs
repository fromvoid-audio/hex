use hex_project::{BeatTime, ClipFlavor, Command, Created, Editor, ElementColor, Project};
use hex_storage::{decode, encode};
use serde_json::{Value, json};

fn fixture() -> (
    Editor,
    hex_project::TrackId,
    hex_project::ClipId,
    hex_project::ClipId,
) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported {
        track, clip: audio, ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "track".into(),
            path: "fixture.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::Clip(midi) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(8).unwrap(),
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("midi")
    };
    editor
        .execute(Command::RenameClip {
            clip: audio,
            name: "audio label".into(),
        })
        .unwrap();
    editor
        .execute(Command::RenameClip {
            clip: midi,
            name: "midi label".into(),
        })
        .unwrap();
    (editor, track, audio, midi)
}

#[test]
fn track_colors_and_both_clip_domains_roundtrip_with_history_and_copies() {
    let (mut editor, track, audio, midi) = fixture();
    for color in ElementColor::ALL {
        let before = editor.project().clone();
        editor
            .execute(Command::SetTrackColor { track, color })
            .unwrap();
        // Every actual color change produces a history step, including Gray.
        if color != before.track(track).unwrap().color() {
            assert!(editor.undo());
            assert_eq!(editor.project(), &before);
            assert!(editor.redo());
        }
        assert_eq!(
            decode(&encode(editor.project()).unwrap()).unwrap(),
            *editor.project()
        );
    }
    for clip in [audio, midi] {
        editor
            .execute(Command::SetClipFlavor {
                clip,
                flavor: ClipFlavor::Accent,
            })
            .unwrap();
    }
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(session_audio) = editor
        .execute(Command::CopyArrangementAudioClipToSession {
            source: audio,
            scene,
        })
        .unwrap()
    else {
        panic!("copy")
    };
    let Created::Scene(scene2) = editor
        .execute(Command::AddScene {
            name: "second".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(session_midi) = editor
        .execute(Command::CopyArrangementMidiToSession {
            source: midi,
            scene: scene2,
        })
        .unwrap()
    else {
        panic!("copy")
    };
    let Created::Clip(right) = editor
        .execute(Command::SplitClip {
            clip: audio,
            frame: 48_000,
        })
        .unwrap()
    else {
        panic!("split")
    };
    let Created::Clip(midi_right) = editor
        .execute(Command::SplitMidiClip {
            clip: midi,
            beat: BeatTime::from_quarters(10).unwrap(),
        })
        .unwrap()
    else {
        panic!("split")
    };
    for clip in [audio, right, midi, midi_right, session_audio, session_midi] {
        assert_eq!(editor.project().clip_flavor(clip), Some(ClipFlavor::Accent));
    }
    let before = editor.project().clone();
    editor
        .execute(Command::SetClipFlavor {
            clip: session_midi,
            flavor: ClipFlavor::Muted,
        })
        .unwrap();
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(
        decode(&encode(editor.project()).unwrap()).unwrap(),
        *editor.project()
    );
    assert!(hex_storage::encode_schema22(editor.project()).is_err());
    assert!(hex_storage::encode_schema20(editor.project()).is_err());
    // Moving keeps flavor and the destination supplies the hue.
    let Created::Track(destination) = editor
        .execute(Command::AddTrack {
            name: "destination".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::SetTrackColor {
            track: destination,
            color: ElementColor::Cyan,
        })
        .unwrap();
    editor
        .execute(Command::MoveClip {
            clip: right,
            track: destination,
            start: 0,
        })
        .unwrap();
    assert_eq!(
        editor.project().track(destination).unwrap().color(),
        ElementColor::Cyan
    );
    assert_eq!(
        editor.project().track(destination).unwrap().clips()[0].flavor(),
        ClipFlavor::Accent
    );
    editor
        .execute(Command::CopyTimeRangeTo {
            range: hex_project::TimeRange::new(vec![track], 0, 288_000).unwrap(),
            track: destination,
            frame: 48_000,
        })
        .unwrap();
    assert!(
        editor
            .project()
            .track(destination)
            .unwrap()
            .clips()
            .iter()
            .all(|c| c.flavor() == ClipFlavor::Accent)
    );
    assert!(
        editor
            .project()
            .track(destination)
            .unwrap()
            .midi_clips()
            .iter()
            .all(|c| c.flavor() == ClipFlavor::Accent)
    );
    // Conversion only supports MIDI-to-audio; flavor survives both locations.
    for clip in [midi_right, session_midi] {
        let flavor = editor.project().clip_flavor(clip);
        let (_, frames) = editor.project().midi_bounce_range(clip).unwrap();
        editor
            .execute(Command::ConvertMidiClipToAudio {
                clip,
                path: format!("bounce-{}.wav", clip.value()).into(),
                frames,
            })
            .unwrap();
        assert_eq!(editor.project().clip_flavor(clip), flavor);
    }
    assert_eq!(
        decode(&encode(editor.project()).unwrap()).unwrap(),
        *editor.project()
    );
}

#[test]
fn current_format_strict_appearance_validation() {
    let (mut editor, track, _, _) = fixture();
    editor
        .execute(Command::SetTrackColor {
            track,
            color: ElementColor::Gray,
        })
        .unwrap();
    let value: Value = serde_json::from_slice(&encode(editor.project()).unwrap()).unwrap();
    assert_eq!(value["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    for (field, bad) in [
        ("track_colors", json!([])),
        ("track_colors", json!([{"track":1,"color":"unknown"}])),
        ("track_colors", json!([{"track":9999,"color":"cyan"}])),
        (
            "track_colors",
            json!([{"track":1,"color":"cyan"},{"track":1,"color":"red"}]),
        ),
        ("accented_clips", json!([9999])),
        ("accented_clips", json!([1])),
        ("accented_clips", json!([3, 3])),
    ] {
        let mut invalid = value.clone();
        invalid[field] = bad;
        assert!(
            decode(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "{field}: {invalid}"
        );
    }
    for field in ["track_colors", "accented_clips"] {
        let mut invalid = value.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
}
