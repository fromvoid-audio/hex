use hex_project::{Command, Created, Editor, Project, TrackDeviceSlot};
use hex_storage::{decode, encode};
#[test]
fn empty_tracks_and_explicit_device_tracks_roundtrip_without_autoloading() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    for command in [
        Command::AddTrack {
            name: "empty audio".into(),
        },
        Command::AddMidiTrack {
            name: "empty midi".into(),
        },
        Command::ImportAudioTrack {
            name: "import".into(),
            path: "not-read.wav".into(),
            sample_rate: 48_000,
            frames: 480,
        },
    ] {
        let id = match editor.execute(command).unwrap() {
            Created::Track(id) | Created::Imported { track: id, .. } => id,
            _ => panic!("track"),
        };
        let after = editor.project().snapshot();
        assert!(editor.project().track(id).unwrap().instrument().is_none());
        assert!(editor.project().track(id).unwrap().devices().is_empty());
        assert!(editor.undo());
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
    let explicit = editor
        .execute(Command::InsertTrackWithDevice {
            name: "explicit".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let Created::Track(id) = explicit else {
        panic!("explicit track")
    };
    let state = editor
        .project()
        .track(id)
        .unwrap()
        .instrument()
        .unwrap()
        .clone();
    let bytes = encode(editor.project()).unwrap();
    for _ in 0..4 {
        let loaded = decode(&bytes).unwrap();
        assert_eq!(&loaded, editor.project());
        assert_eq!(loaded.track(id).unwrap().instrument(), Some(&state));
        assert!(
            loaded
                .tracks()
                .iter()
                .filter(|t| t.id() != id)
                .all(|t| t.ordered_device_chain().count() == 0)
        );
        assert_eq!(encode(&loaded).unwrap(), bytes);
    }
}
