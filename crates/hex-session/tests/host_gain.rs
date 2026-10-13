use hex_engine::{DEVICE_OUTPUT_GAIN, PlaybackCommand, playback::Playback};
use hex_lattice_contract::ParameterValue;
use hex_project::{Command, Created};
use hex_session::Session;

#[test]
fn instrument_host_trim_is_undoable_and_preserves_synthesis_identity() {
    let mut session = Session::new(48_000, 64, 0).unwrap();
    let (Created::Track(track), graph) = session
        .command_prepared_revision(
            Command::AddMidiTrack {
                name: "gain staging".into(),
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
            DEVICE_OUTPUT_GAIN.id,
            ParameterValue::Float(-12.0),
            Some(&graph),
        )
        .unwrap();
    let changed = session.project().device(device).unwrap().state().clone();
    assert!(original.processing_eq(&changed));
    assert_eq!(changed.output_gain_db(), -12.0);
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
                DEVICE_OUTPUT_GAIN.id,
                ParameterValue::Float(f32::NAN),
                Some(&changed_graph)
            )
            .is_err()
    );
    assert_eq!(session.project().device(device).unwrap().state(), &changed);
}
