//! A refused public model batch cannot prepare/publish over last valid playback.
use hex_engine::{
    PreparedRevision,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{BeatTime, Command, Created, Editor, Project, RoutingOwner, TrackId};
use hex_session::{Session, SessionError};
use std::sync::Arc;

fn track(editor: &mut Editor) -> TrackId {
    let Created::Track(id) = editor
        .execute(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    id
}
fn compile(project: &Project) -> Result<Arc<PreparedRevision>, SessionError> {
    Session::from_loaded(project.clone(), 4, 0, Vec::new())?.compile_prepared_revision(None)
}
fn playing(revision: Arc<PreparedRevision>) -> Playback {
    let mut playback = Playback::from_prepared(revision);
    playback.apply(PlaybackCommand::PlayFrom(0));
    playback
}

#[test]
fn invalid_public_batches_never_prepare_or_replace_continuing_valid_audio() {
    for audio_cycle in [true, false] {
        let mut setup = Editor::new(Project::new(8_000).unwrap(), 8).unwrap();
        let source = track(&mut setup);
        let destination = track(&mut setup);
        let Created::Clip(clip) = setup
            .execute(Command::PlaceMidiClip {
                track: source,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        setup
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let invalid = if audio_cycle {
            let Created::ReturnTrack(r1) = setup
                .execute(Command::AddReturnTrack { name: "r1".into() })
                .unwrap()
            else {
                panic!("return")
            };
            let Created::ReturnTrack(r2) = setup
                .execute(Command::AddReturnTrack { name: "r2".into() })
                .unwrap()
            else {
                panic!("return")
            };
            setup
                .execute(Command::SetReturnFeed {
                    source: r1,
                    destination: r2,
                    gain: 1.0,
                })
                .unwrap();
            setup
                .execute(Command::SetAudioFeed {
                    source: RoutingOwner::Return(r2),
                    destination: source,
                    gain: 1.0,
                })
                .unwrap();
            Command::SetReturnSend {
                track: source,
                return_track: r1,
                gain: 1.0,
            }
        } else {
            setup
                .execute(Command::SetMidiSource {
                    destination,
                    source: Some(source),
                })
                .unwrap();
            Command::RemoveInstrument { track: destination }
        };
        let before = setup.project().clone();
        let mut editor = Editor::new(before.clone(), 4).unwrap();
        let audible = compile(&before).unwrap();
        let mut oracle = playing(Arc::clone(&audible));
        let mut expected = [[0.0; 2]; 128];
        oracle.render(&mut expected).unwrap();
        assert!(expected.iter().any(|frame| frame[0].abs() > 0.001));
        let mut current = playing(Arc::clone(&audible));
        let mut first = [[0.0; 2]; 64];
        current.render(&mut first).unwrap();
        assert_eq!(first, expected[..64]);
        let mut preparations = 0;
        let result = editor.execute_many_prepared(vec![invalid], |candidate, _| {
            preparations += 1;
            compile(candidate)
        });
        // This is the normal caller publication boundary: only success supplies
        // a replacement. A compiler's separate refusal is not model validation.
        match result {
            Ok((_, revision)) => current = playing(revision),
            Err(error) => assert!(matches!(error, SessionError::Project(_))),
        }
        assert_eq!(preparations, 0);
        assert_eq!(editor.project(), &before);
        assert!(!editor.can_undo() && !editor.can_redo());
        assert!(Arc::ptr_eq(current.prepared_revision().unwrap(), &audible));
        let mut next = [[0.0; 2]; 64];
        current.render(&mut next).unwrap();
        assert_eq!(
            next,
            expected[64..],
            "audio must continue, not restart or silence"
        );
    }
}
