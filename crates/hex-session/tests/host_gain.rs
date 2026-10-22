use hex_engine::{DEVICE_INPUT_GAIN, DEVICE_OUTPUT_GAIN, PlaybackCommand, playback::Playback};
use hex_lattice_contract::ParameterValue;
use hex_project::{Command, Created};
use hex_session::Session;

#[test]
fn instrument_host_trim_is_undoable_and_preserves_synthesis_identity() {
    for parameter in [DEVICE_INPUT_GAIN.id, DEVICE_OUTPUT_GAIN.id] {
        let mut session = Session::new(48_000, 64, 0).unwrap();
        let (Created::Track(track), graph) = session
            .command_prepared_revision(
                Command::InsertTrackWithDevice {
                    name: "gain staging".into(),
                    before_track: None,
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    state: hex_project::default_instrument_state()
                        .expect("fixture instrument state"),
                },
                None,
            )
            .unwrap()
        else {
            panic!("track");
        };
        let device = session
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let original = session.project().device(device).unwrap().state().clone();
        let changed_graph = session
            .commit_device_parameter(
                device,
                parameter,
                ParameterValue::Float(-12.0),
                Some(&graph),
            )
            .unwrap();
        let changed = session.project().device(device).unwrap().state().clone();
        assert!(original.processing_eq(&changed));
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                &changed,
                parameter
            )
            .unwrap(),
            ParameterValue::Float(-12.0)
        );
        // Gain edits preserve the prepared source identity: no instrument replacement.
        let mut old = Playback::try_from_prepared_with_session(graph).unwrap();
        old.apply(PlaybackCommand::Play);
        let mut new = Playback::try_from_prepared_with_session(changed_graph.clone()).unwrap();
        new.try_inherit_runtime_from(&mut old).unwrap();
        let undone = session
            .undo_prepared_revision(Some(&changed_graph))
            .unwrap()
            .unwrap();
        assert_eq!(session.project().device(device).unwrap().state(), &original);
        session
            .redo_prepared_revision(Some(&undone))
            .unwrap()
            .unwrap();
        assert_eq!(session.project().device(device).unwrap().state(), &changed);
        assert!(
            session
                .commit_device_parameter(
                    device,
                    parameter,
                    ParameterValue::Float(f32::NAN),
                    Some(&changed_graph)
                )
                .is_err()
        );
        assert_eq!(session.project().device(device).unwrap().state(), &changed);
    }
}
