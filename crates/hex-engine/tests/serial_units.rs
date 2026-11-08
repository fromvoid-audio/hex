use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn arrangement_and_session_refuse_serial_units_and_preserve_the_previous_revision() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    for _ in 0..2 {
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_devices::utility::UTILITY_DEFINITION
                    .default_state()
                    .unwrap(),
            })
            .unwrap();
    }
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "memory/serial.wav".into(),
            sample_rate: 48_000,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 16,
            source_offset: 0,
            source_length: 16,
            looping: false,
        })
        .unwrap();
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48_000, vec![[0.25, -0.5]; 16]).unwrap(),
    )]);
    assert!(hex_engine::compile::project(editor.project(), &media).is_ok());
    let previous = Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let before = editor.project().snapshot();
    let mut snapshot = before.clone();
    snapshot.main_devices[1].parent = Some(snapshot.main_devices[0].id);
    let candidate = Project::from_snapshot(snapshot).unwrap();
    let error = hex_engine::compile::project(&candidate, &media)
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("serial device units are not prepared")
    );
    assert!(
        hex_engine::PreparedRevision::prepare(
            &candidate,
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .is_err()
    );
    assert!(
        hex_engine::PreparedSessionInventory::prepare(
            &candidate,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    let mut playback = hex_engine::SessionPlayback::new(previous).unwrap();
    playback
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let mut output = [[1.0; 2]; 16];
    playback.render(&mut output).unwrap();
    assert_eq!(output, [[0.25, -0.5]; 16]);
}
