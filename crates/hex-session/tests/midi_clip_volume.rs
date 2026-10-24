use hex_project::{
    BeatTime, Command, Created, Editor, MidiClipGain, MidiClipVolume, MidiClipVolumePoint,
};
use hex_session::Session;
use std::collections::BTreeMap;
fn volume() -> MidiClipVolume {
    MidiClipVolume::new(
        vec![MidiClipVolumePoint::linear(
            BeatTime::ZERO,
            MidiClipGain::new(0.5).unwrap(),
        )],
        BeatTime::ONE,
    )
    .unwrap()
}
#[test]
fn unsupported_volume_refuses_before_session_commit_and_each_public_compiler_path() {
    for console in [false, true] {
        let mut session = Session::new(48_000, 16, 4096).unwrap();
        let Created::Track(track) = session
            .command(Command::AddTrack {
                name: "volume".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let clip = if console {
            let Created::Scene(scene) = session
                .command(Command::AddScene {
                    name: "scene".into(),
                })
                .unwrap()
            else {
                panic!("scene")
            };
            let Created::SessionClip(clip) = session
                .command(Command::CreateSessionMidiClip {
                    track,
                    scene,
                    name: "clip".into(),
                    source_length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("console")
            };
            clip
        } else {
            let Created::Clip(clip) = session
                .command(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("clip")
            };
            clip
        };
        session
            .command(Command::AddTrack {
                name: "redo sentinel".into(),
            })
            .unwrap();
        assert!(session.undo());
        assert!(session.can_redo());
        let before = session.project().clone();
        let error = session
            .command(Command::ReplaceMidiClipVolume {
                clip,
                volume: volume(),
            })
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("midi clip volume playback is not prepared"),
            "{error}"
        );
        assert_eq!(session.project(), &before);
        assert!(session.can_redo());
        // The canonical/storage owner can retain the curve; every current
        // playback ingress must refuse until its audible consumer exists.
        let mut metadata = Editor::new(before, 4).unwrap();
        metadata
            .execute(Command::ReplaceMidiClipVolume {
                clip,
                volume: volume(),
            })
            .unwrap();
        assert!(hex_engine::compile::project(metadata.project(), &BTreeMap::new()).is_err());
        assert!(
            hex_engine::PreparedSessionInventory::prepare(
                metadata.project(),
                session.sine_table(),
                None
            )
            .is_err()
        );
        assert!(
            hex_engine::PreparedRevision::prepare(
                metadata.project(),
                &BTreeMap::new(),
                session.sine_table(),
                None
            )
            .is_err()
        );
        assert!(session.redo());
    }
}
