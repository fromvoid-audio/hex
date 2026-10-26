use super::*;
use hex_project::{BeatTime, Command, Created};

#[test]
fn stopped_zero_compensation_handoff_keeps_transient_cue_origin_instead_of_relaunching_zero() {
    let mut f = fixture(&[&[7]], 32);
    let track = f.tracks[0];
    f.editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let Created::Scene(scene) = f
        .editor
        .execute(Command::AddScene { name: "cue".into() })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = f
        .editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "cue".into(),
            source_length: BeatTime::from_quarters(2).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    f.editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ONE,
            duration: BeatTime::ONE,
            pitch: 64,
            channel: 0,
            velocity: 93,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    assert!(old.compensation.is_some());
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::CueMidiClip {
        track,
        clip,
        source_beat: BeatTime::ONE,
        expected_route_generation: old.route_state_generation(),
    })
    .unwrap();
    let origin = old.route_status(0).unwrap().active_origin;
    let mut next = SessionPlayback::new(revision).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(
        next.route_status(0).unwrap().active_origin,
        origin,
        "quiescent fresh delay/processor custody preserves the cued source coordinate"
    );
    assert_eq!(origin.unwrap().elapsed_at_anchor, BeatTime::ONE);
    assert!(!next.playing());
    next.apply(SessionPlaybackCommand::Play).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 64];
    let mut expected = [[0.0; 2]; 64];
    next.render(&mut actual).unwrap();
    old.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|v| v.abs() > 1e-5));
}
