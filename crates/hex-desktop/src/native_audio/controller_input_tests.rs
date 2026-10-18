//! Same-revision resident publications use the actual borrowed client/worker bridge.
use super::*;
use hex_engine::{
    computer_keyboard::ComputerKeyboardInput,
    live_expression::{LiveExpressionInput, Snapshot},
    playback::Playback,
};

fn forward(harness: &mut Harness, playback: Playback) -> u64 {
    let generation = harness.context.unwrap().project_generation;
    let client = &mut harness.audio.as_mut().unwrap().control;
    let ticket = client
        .forward_resident_publication(
            playback,
            hex_audio::prepared::PublicationPolicy::InheritRuntime,
            generation,
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(response) = harness
            .audio
            .as_mut()
            .unwrap()
            .control
            .try_resident_publication_response()
        {
            assert_eq!(response.token, ticket);
            return response.result.unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "resident publication worker did not reply"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn resident_controller_and_keyboard_publications_keep_source_identity_through_worker_bridge() {
    let mut harness = Harness::new();
    harness.next();
    harness.open();
    assert_eq!(
        harness.audio.as_ref().unwrap().mode,
        DocumentAudioMode::Resident
    );
    let added = harness.run(Request::Edit(hex_project::Command::AddMidiTrack {
        name: "vector".into(),
    }));
    let track = added.state.project.tracks()[0].id();
    let instrument = added
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let class = hex_engine::NativeDeviceClass::Instrument;
    let vector = hex_engine::native_device_catalog_for_class(class)
        .unwrap()
        .into_iter()
        .find(|entry| entry.type_id.as_str() == Ok("hex.instrument.operator"))
        .unwrap()
        .type_id;
    let state = hex_engine::default_native_device_state_for_class(class, vector).unwrap();
    let edited = harness.run(Request::Edit(hex_project::Command::ReplaceDeviceState {
        device: instrument,
        state,
    }));
    let generation = harness.context.unwrap().project_generation;
    let keys = Arc::new(ComputerKeyboardInput::default());
    let oracle_keys = Arc::new(ComputerKeyboardInput::default());
    for input in [&keys, &oracle_keys] {
        input.set_key(0, Some((60, 100, 1))).unwrap();
    }
    let mut keyboard = harness.run(Request::PrepareComputerKeyboard {
        project_generation: generation,
        revision: edited.state.revision,
        request_id: 1,
        track,
        input: Arc::clone(&keys),
    });
    let playback = keyboard.playback.take().unwrap();
    let source = Arc::clone(playback.prepared_revision().unwrap());
    let mut oracle = Playback::from_prepared(Arc::clone(&source));
    oracle
        .prepare_computer_keyboard(&edited.state.project, track, Arc::clone(&oracle_keys))
        .unwrap();
    let submitted = forward(&mut harness, playback);
    let actual = harness.render();
    let mut expected = [[0.0; 2]; 64];
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        harness
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        submitted
    );
    let controllers = Arc::new(LiveExpressionInput::new(generation, track, instrument));
    let mut activation = harness.run(Request::PrepareLiveExpression {
        input: Arc::clone(&controllers),
        revision: edited.state.revision,
    });
    assert!(Arc::ptr_eq(&activation.state, &keyboard.state));
    let playback = activation.playback.take().unwrap();
    assert!(
        Arc::ptr_eq(playback.prepared_revision().unwrap(), &source),
        "the borrowed resident bridge must forward the same immutable Session artifact"
    );
    assert!(Arc::ptr_eq(
        activation.live_expression_complete.as_ref().unwrap(),
        &controllers
    ));
    oracle
        .prepare_live_expression(&edited.state.project, Arc::clone(&controllers))
        .unwrap();
    let submitted = forward(&mut harness, playback);
    let actual = harness.render();
    oracle.render(&mut expected).unwrap();
    assert_eq!(
        actual, expected,
        "controller publication must preserve held mutable keyboard history"
    );
    assert!(!keys.is_cancelled());
    assert_eq!(
        harness
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        submitted
    );
    controllers.publish(Snapshot::new(80, 4096, 100).unwrap());
    let actual = harness.render();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    keys.set_key(0, None).unwrap();
    oracle_keys.set_key(0, None).unwrap();
    let actual = harness.render();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(!keys.is_cancelled());
}

#[test]
fn dedicated_tuning_audition_uses_resident_worker_bridge_without_a_selected_track() {
    let mut harness = Harness::new();
    let initial = harness.next();
    harness.open();
    let generation = harness.context.unwrap().project_generation;
    let input = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(
        generation,
    ));
    let oracle_input = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(
        generation,
    ));
    for input in [&input, &oracle_input] {
        input.set_key(0, Some((60, 100, 1))).unwrap();
    }
    let mut activated = harness.run(Request::PrepareTuningAudition {
        revision: initial.state.revision,
        input: Arc::clone(&input),
    });
    assert_eq!(activated.state.project, initial.state.project);
    assert_eq!(activated.state.revision, initial.state.revision);
    assert!(Arc::ptr_eq(
        activated.tuning_audition_complete.as_ref().unwrap(),
        &input
    ));
    let playback = activated.playback.take().unwrap();
    let mut oracle = Playback::from_prepared(Arc::clone(playback.prepared_revision().unwrap()));
    oracle
        .prepare_tuning_audition(&initial.state.project, oracle_input)
        .unwrap();
    let revision = forward(&mut harness, playback);
    let actual = harness.render(); // actual adapter callback also measures allocations
    let mut expected = [[0.0; 2]; 64];
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| sample.abs() > 0.00001));
    assert_eq!(
        harness
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        revision
    );
    input.cancel();
    assert_eq!(harness.render(), [[0.0; 2]; 64]);
}
