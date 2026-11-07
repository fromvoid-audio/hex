//! Work sampling is observational across ordinary/typed routing and group ownership.
use super::*;
use crate::PreparedPlaybackObservation;

#[test]
fn rotating_work_is_revision_local_and_preserves_audio_midi_groups_and_returns() {
    for groups in [false, true] {
        let mut editor = Editor::new(audio_fixture_project(true, false, false), 8).unwrap();
        if groups {
            let child = editor.project().tracks()[0].id();
            for name in ["inner", "outer"] {
                let Created::Track(group) = editor
                    .execute(Command::GroupTracks {
                        tracks: hex_project::TrackSelectionIds::new(&[child]).unwrap(),
                        name: name.into(),
                    })
                    .unwrap()
                else {
                    panic!("group");
                };
                assert!(editor.project().track(group).unwrap().is_group());
            }
        }
        let prepared = prepare_audio(editor.project());
        let observation = PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap();
        let owners = observation
            .render_work_meters()
            .map(|(owner, _)| owner)
            .collect::<Vec<_>>();
        assert_eq!(owners.len(), editor.project().tracks().len() + 1);
        assert!(
            observation
                .render_work_meters()
                .all(|(_, meter)| meter.sample().is_none())
        );
        let mut tested = linear(prepared);
        let mut oracle = linear(prepare_audio(editor.project()));
        tested.set_device_metering(true);
        oracle.set_device_metering(true);
        observation.set_render_work_enabled(true);
        for (index, _) in owners.iter().enumerate() {
            let mut actual = [[0.0; 2]; 32];
            let mut expected = actual;
            tested.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            assert_eq!(actual, expected);
            for (slot, (owner, meter)) in observation.render_work_meters().enumerate() {
                assert_eq!(owner, owners[slot]);
                assert_eq!(meter.sample().is_some(), slot <= index, "owner {owner:?}");
                if slot == index {
                    assert_eq!(meter.sample().unwrap().sequence, 1);
                }
            }
        }
        let moved = editor.project().tracks().last().unwrap().id();
        let before = editor.project().tracks()[0].id();
        if !groups {
            editor
                .execute(Command::RenameTrack {
                    track: moved,
                    name: "renamed".into(),
                })
                .unwrap();
            editor
                .execute(Command::MoveTrackBefore {
                    track: moved,
                    before: Some(before),
                })
                .unwrap();
        }
        let replacement = prepare_audio(editor.project());
        let replacement_observation =
            PreparedPlaybackObservation::prepare_on_control(&replacement).unwrap();
        assert_eq!(
            replacement_observation
                .render_work_meters()
                .filter_map(|(owner, _)| match owner {
                    OutputCaptureId::Track(id) => Some(id),
                    OutputCaptureId::Return(_) => None,
                })
                .collect::<Vec<_>>(),
            editor
                .project()
                .tracks()
                .iter()
                .map(|track| track.id())
                .collect::<Vec<_>>()
        );
        assert!(
            replacement_observation
                .render_work_meters()
                .all(|(_, meter)| meter.sample().is_none())
        );
        observation.set_render_work_enabled(false);
        assert!(
            observation
                .render_work_meters()
                .all(|(_, meter)| meter.sample().is_none())
        );
    }
    // Analytic MIDI uses its metered source path without changing PCM.
    let project = project(0.5, 0, 8 << 32);
    let prepared = revision(&project, Arc::new(hex_devices::SineTable::new()));
    let observation = PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap();
    observation.set_render_work_enabled(true);
    let mut tested = linear(prepared);
    let mut oracle = linear(revision(&project, Arc::new(hex_devices::SineTable::new())));
    tested.set_device_metering(true);
    oracle.set_device_metering(true);
    let mut actual = [[0.0; 2]; 257];
    let mut expected = actual;
    tested.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    assert!(
        observation
            .render_work_meters()
            .all(|(_, meter)| meter.sample().is_some())
    );
}

#[test]
fn refused_empty_and_stopped_renders_cannot_publish_fresh_zero_work() {
    let prepared = prepare_audio(&audio_fixture_project(false, false, false));
    let observation = PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap();
    observation.set_render_work_enabled(true);
    let mut runtime = SessionPlayback::new(prepared).unwrap();
    runtime.render(&mut [[0.0; 2]; 32]).unwrap();
    runtime.render(&mut []).unwrap();
    assert!(
        observation
            .render_work_meters()
            .all(|(_, meter)| meter.sample().is_none())
    );
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    // An enabled endpoint must not let another preparation runtime publish.
    runtime.render(&mut [[0.0; 2]; 32]).unwrap();
    assert!(
        observation
            .render_work_meters()
            .all(|(_, meter)| meter.sample().is_none())
    );
    runtime.set_device_metering(true);
    runtime.arrangement_frame = u64::MAX - 8;
    assert!(runtime.render(&mut [[0.0; 2]; 32]).is_err());
    assert!(
        observation
            .render_work_meters()
            .all(|(_, meter)| meter.sample().is_none())
    );
}

#[path = "render_work_tests/profile.rs"]
mod profile;
