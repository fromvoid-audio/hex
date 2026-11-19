use hex_project::{BeatTime, Command, Created, MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
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
fn clip_volume_prepares_through_session_and_public_compilers_with_atomic_refusal_and_history() {
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
        let too_long = BeatTime::from_quarters(2).unwrap();
        let invalid = MidiClipVolume::new(
            vec![MidiClipVolumePoint::linear(too_long, MidiClipGain::UNITY)],
            too_long,
        )
        .unwrap();
        assert!(
            session
                .command(Command::ReplaceMidiClipVolume {
                    clip,
                    volume: invalid
                })
                .is_err()
        );
        assert_eq!(session.project(), &before);
        assert!(session.can_redo(), "refusal preserves redo");
        session
            .command(Command::ReplaceMidiClipVolume {
                clip,
                volume: volume(),
            })
            .unwrap();
        let authored = session.project().clone();
        assert!(
            !session.can_redo(),
            "one admitted edit replaces the redo branch"
        );
        assert!(hex_engine::compile::project(session.project(), &BTreeMap::new()).is_ok());
        assert!(
            hex_engine::PreparedSessionInventory::prepare(
                session.project(),
                session.sine_table(),
                None
            )
            .is_ok()
        );
        assert!(
            hex_engine::PreparedRevision::prepare(
                session.project(),
                &BTreeMap::new(),
                session.sine_table(),
                None
            )
            .is_ok()
        );
        assert!(session.undo());
        assert_eq!(session.project(), &before);
        assert!(session.redo());
        assert_eq!(session.project(), &authored);
    }
}
