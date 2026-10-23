//! Transaction proof for the admitted feed kernel, using private clock fault injection.
use super::*;
use hex_project::{Command, Created, Editor, Project};
use std::collections::BTreeMap;

#[test]
fn late_chunk_failure_rolls_back_feed_histories_clocks_routes_and_output() {
    assert_late_failure(false);
}
#[test]
fn late_chunk_failure_with_gain_automation_rolls_back_feed_histories_clocks_routes_and_output() {
    assert_late_failure(true);
}
fn assert_late_failure(automation: bool) {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "source".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48000,
            source_frames: 1024,
            playback_frames: 1024,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let mut returns = Vec::new();
    for _ in 0..2 {
        let Created::ReturnTrack(id) = editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return")
        };
        returns.push(id);
        let state = crate::patch_native_device_parameter(
            &crate::default_native_device_state(crate::DeviceTypeId::from_static("hex.utility"))
                .unwrap(),
            hex_lattice_contract::ParameterId::new(11).unwrap(),
            hex_lattice_contract::ParameterValue::Bool(true),
        )
        .unwrap();
        editor
            .execute(Command::InsertReturnDevice {
                return_track: id,
                before: None,
                state,
            })
            .unwrap();
    }
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: returns[0],
            gain: 1.0,
        })
        .unwrap();
    editor
        .execute(Command::SetReturnFeed {
            source: returns[0],
            destination: returns[1],
            gain: 0.5,
        })
        .unwrap();
    if automation {
        editor
            .execute(Command::ReplaceTrackGainEnvelope {
                track,
                points: vec![
                    hex_project::GainPoint {
                        incoming_gain: None,
                        beat: hex_project::BeatTime::ZERO,
                        gain: 0.5,
                        outgoing: hex_project::GainInterpolation::Linear,
                    },
                    hex_project::GainPoint {
                        incoming_gain: None,
                        beat: hex_project::BeatTime::ONE,
                        gain: 1.0,
                        outgoing: hex_project::GainInterpolation::Step,
                    },
                ],
            })
            .unwrap();
    }
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::from([(
                asset,
                crate::Audio::new(48000, vec![[1.0; 2]; 1024]).unwrap(),
            )]),
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut tested = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut oracle = SessionPlayback::new(revision).unwrap();
    for playback in [&mut tested, &mut oracle] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut [[0.0; 2]; 4]).unwrap();
    }
    let before_performance = tested.performance_frame;
    let before_routes = tested.routes.clone();
    let before_suppressions = tested.suppressions.clone();
    tested.arrangement_frame = u64::MAX - 300;
    let mut output = [[9.0; 2]; 512];
    let mut observed = 0;
    assert_eq!(
        tested.render_with_post_node_observer(
            &mut output,
            Some(&mut |_, _, _: &[Frame]| observed += 1)
        ),
        Err(SessionPlaybackError::Arrangement("render range overflow"))
    );
    assert!(
        observed > 0,
        "first chunk must execute before the injected late failure"
    );
    assert_eq!(output, [[0.0; 2]; 512]);
    assert_eq!(tested.performance_frame, before_performance);
    assert_eq!(tested.arrangement_frame, u64::MAX - 300);
    assert_eq!(tested.routes, before_routes);
    assert_eq!(tested.suppressions, before_suppressions);
    tested.arrangement_frame = 4;
    let mut expected = [[0.0; 2]; 17];
    let mut actual = expected;
    oracle.render(&mut expected).unwrap();
    tested.render(&mut actual).unwrap();
    assert_eq!(
        actual, expected,
        "no speculative source or destination filter history may commit"
    );
}
