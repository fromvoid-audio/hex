//! Timed resident Playback entry point, measured directly, not a device callback.
use hex_engine::{
    Audio, PlaybackCommand, PreparedRevision, SineTable, TimedTransportCommand, playback::Playback,
};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn timed_transport_render_does_not_allocate_or_free() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 1024,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25; 2]; 1024]).unwrap())]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let mut playback = Playback::from_prepared(revision);
    let commands = [
        TimedTransportCommand {
            offset: 0,
            command: PlaybackCommand::PlayFrom(32),
        },
        TimedTransportCommand {
            offset: 16,
            command: PlaybackCommand::Seek(64),
        },
        TimedTransportCommand {
            offset: 32,
            command: PlaybackCommand::Stop,
        },
        TimedTransportCommand {
            offset: 48,
            command: PlaybackCommand::PlayFrom(128),
        },
    ];
    let mut output = [[0.0; 2]; 64];
    let mut result = Ok(());
    let counts = allocation_counter::measure(|| {
        result = playback.render_with_timed_transport(&mut output, &commands);
    });
    assert!(result.is_ok());
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(&output[..32], &[[0.25; 2]; 32]);
    assert_eq!(&output[32..48], &[[0.0; 2]; 16]);
    assert_eq!(&output[48..], &[[0.25; 2]; 16]);
}
