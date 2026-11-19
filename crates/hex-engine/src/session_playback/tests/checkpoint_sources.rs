use super::*;

fn shared_source_fixture() -> (
    Arc<PreparedRevision>,
    hex_project::TrackId,
    hex_project::TrackId,
) {
    let instrument_id =
        hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let base = with_test_instrument(&project(1.0, 0, 8 << 32), instrument_id, 0);
    let receiver = base.tracks()[0].id();
    let source = base.tracks()[0].source_id().unwrap();
    let mut editor = Editor::new(base, 32).unwrap();
    let Created::Track(contributor) = editor
        .execute(Command::AddMidiTrack {
            name: "checkpoint contributor".into(),
        })
        .unwrap()
    else {
        panic!("contributor track")
    };
    editor
        .execute(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        })
        .unwrap();
    let project = editor.project().clone();

    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: instrument_id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let revision = revision_with_instrument_registry(
        &project,
        Arc::new(hex_devices::SineTable::new()),
        &registry,
    );
    (revision, receiver, contributor)
}

fn process_source_input(
    runtime: &mut MutableInstrumentTrackRuntime,
    input: crate::mutable_instrument::LiveInputId,
    transitions: &[hex_lattice::NativeInstrumentEvent],
    frame_count: usize,
) -> Vec<crate::Frame> {
    runtime.begin_block();
    runtime.stage_live_events(input, transitions, None).unwrap();
    let mut frames = vec![[0.0; 2]; frame_count];
    runtime
        .process_source_segment(
            0,
            &mut frames,
            &mut MutableOnsetBuffer::default(),
            &[],
            &mut Vec::new(),
            None,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
            None,
            None,
        )
        .unwrap();
    runtime.commit_block();
    frames
}

#[test]
fn shared_source_checkpoint_preserves_live_age_and_occurrence_identity() {
    let (revision, receiver, contributor) = shared_source_fixture();
    let mut source = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut target = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let source_index = source
        .routes
        .iter()
        .position(|route| route.id == receiver)
        .unwrap();
    let target_index = target
        .routes
        .iter()
        .position(|route| route.id == receiver)
        .unwrap();
    let source_runtime = source.mutable_instruments[source_index].as_mut().unwrap();
    let target_runtime = target.mutable_instruments[target_index].as_ref().unwrap();
    assert!(source_runtime.is_source_owner());
    assert!(target_runtime.is_source_owner());

    let input = crate::mutable_instrument::LiveInputId {
        track: contributor,
        generation: 19,
    };
    let note_on = [hex_lattice::NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 7,
        pitch: 60,
        velocity: 100,
    }];
    let prefix = process_source_input(source_runtime, input, &note_on, 4);
    assert!(prefix.iter().any(|frame| frame[0] != 0.0));
    assert_eq!(source_runtime.next_occurrence_id(), 1);
    assert_eq!(source_runtime.source_live_count(), 1);

    let factory = revision
        .session()
        .mutable_instrument_factory(receiver)
        .unwrap();
    let base_charge = MutableInstrumentTrackRuntime::checkpoint_storage_upper_bound(
        factory,
        revision.arrangement().sample_rate(),
    )
    .unwrap();
    let live_charge = MutableInstrumentTrackRuntime::source_live_storage_upper_bound();
    let mut checkpoints = MutableRuntimeCheckpointStore::new();
    checkpoints.capture_track(&source, contributor).unwrap();
    assert_eq!(
        checkpoints.declared_storage_bytes(),
        std::mem::size_of::<MutableRuntimeCheckpointStore>() + base_charge + live_charge
    );
    assert_eq!(
        checkpoints.capture_track(&source, receiver),
        Err(MutableRuntimeCheckpointError::DuplicateCheckpoint),
        "contributor and receiver requests address one source checkpoint"
    );

    checkpoints.restore_track(&mut target, receiver).unwrap();
    let restored = target.mutable_instruments[target_index].as_mut().unwrap();
    assert!(restored.is_source_owner());
    assert_eq!(restored.next_occurrence_id(), 1);
    assert_eq!(restored.source_live_count(), 1);

    let note_off = [hex_lattice::NativeInstrumentEvent::NoteOff {
        frame_offset: 0,
        occurrence_id: 7,
    }];
    let expected = process_source_input(
        source.mutable_instruments[source_index].as_mut().unwrap(),
        input,
        &note_off,
        20,
    );
    let actual = process_source_input(restored, input, &note_off, 20);
    assert_eq!(actual, expected);
    assert_eq!(
        restored.source_live_count(),
        source.mutable_instruments[source_index]
            .as_ref()
            .unwrap()
            .source_live_count()
    );
    let expected_expiry = process_source_input(
        source.mutable_instruments[source_index].as_mut().unwrap(),
        input,
        &[],
        1,
    );
    let actual_expiry = process_source_input(restored, input, &[], 1);
    assert_eq!(actual_expiry, expected_expiry);
    assert_eq!(restored.source_live_count(), 0);
}
