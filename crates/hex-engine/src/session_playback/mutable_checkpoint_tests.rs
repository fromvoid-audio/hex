use super::*;

#[test]
pub(super) fn mutable_checkpoint_restores_held_and_released_feedback_with_owned_occurrence_ids() {
    for (duration, prefix_frames, expect_released) in
        [(8 << 32, 19, false), ((1 << 32) / 6_000, 5, true)]
    {
        let (revision, track) = mutable_checkpoint_fixture(duration);
        let mut source = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut restored = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        for playback in [&mut source, &mut restored] {
            playback.apply(SessionPlaybackCommand::Play).unwrap();
        }
        let prefix = render_checkpoint_prefix(&mut source, prefix_frames);
        let restored_prefix = render_checkpoint_prefix(&mut restored, prefix_frames);
        assert_eq!(prefix, restored_prefix);
        assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

        let source_runtime = source.mutable_instruments[0].as_ref().unwrap();
        let next_id = source_runtime.next_occurrence_id();
        assert_eq!(next_id, 1);
        let proof = source_runtime.occurrence_proofs().first().unwrap();
        assert_eq!(proof.id, next_id);
        assert_eq!(proof.note_off_sent, expect_released);
        assert!(source_runtime.committed_feedback_tail_is_active());

        let mut checkpoints = MutableRuntimeCheckpointStore::new();
        checkpoints.capture_track(&source, track).unwrap();
        assert_eq!(checkpoints.checkpoint_count(), 1);
        assert!(checkpoints.declared_storage_bytes() > 0);
        assert!(checkpoints.declared_storage_bytes() <= MAX_MUTABLE_RUNTIME_CHECKPOINT_STORE_BYTES);

        // Make the target's device state different while retaining its exact
        // revision and musical frame. Restore must recover both the voice
        // history and the engine's occurrence-ID ledger.
        reset_checkpoint_track(&mut restored, track);
        assert_eq!(
            restored.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .next_occurrence_id(),
            0
        );
        checkpoints.restore_track(&mut restored, track).unwrap();
        assert_eq!(
            restored.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .next_occurrence_id(),
            next_id
        );
        assert_eq!(
            restored.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .occurrence_proofs(),
            source_runtime.occurrence_proofs()
        );
        checkpoints.restore_track(&mut source, track).unwrap();
        assert_eq!(
            source.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .next_occurrence_id(),
            next_id
        );

        let expected = render_checkpoint_partitioned(&mut source, &[1, 7, 3, 11, 2], 48);
        let actual = render_checkpoint_partitioned(&mut restored, &[5, 1, 9, 2, 7], 48);
        assert_eq!(actual, expected);
        // The same checkpoint remains reusable and the next occurrence still
        // receives the same ID after an identical continuation.
        let id_after = source.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .next_occurrence_id();
        assert_eq!(
            restored.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .next_occurrence_id(),
            id_after
        );
    }
}

#[test]
fn mutable_checkpoint_refuses_uncommitted_wrong_revision_and_wrong_frame_atomically() {
    let (revision, track) = mutable_checkpoint_fixture(8 << 32);
    let mut source = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut target = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    for playback in [&mut source, &mut target] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        render_checkpoint_prefix(playback, 16);
    }
    let mut checkpoints = MutableRuntimeCheckpointStore::new();
    checkpoints.capture_track(&source, track).unwrap();

    let wrong_frame_before = target.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .next_occurrence_id();
    render_checkpoint_prefix(&mut target, 1);
    assert_eq!(
        checkpoints.restore_track(&mut target, track),
        Err(MutableRuntimeCheckpointError::IncompatiblePerformanceFrame)
    );
    assert_eq!(
        target.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .next_occurrence_id(),
        wrong_frame_before
    );

    let instrument_id =
        hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: instrument_id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let changed_project = with_test_instrument(&project(1.0, 0, 8 << 32), instrument_id, 1);
    let changed_revision = revision_with_instrument_registry(
        &changed_project,
        Arc::new(hex_devices::SineTable::new()),
        &registry,
    );
    let mut wrong_revision = SessionPlayback::new(changed_revision).unwrap();
    wrong_revision.apply(SessionPlaybackCommand::Play).unwrap();
    render_checkpoint_prefix(&mut wrong_revision, 16);
    let wrong_revision_id = wrong_revision.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .next_occurrence_id();
    assert_eq!(
        checkpoints.restore_track(&mut wrong_revision, track),
        Err(MutableRuntimeCheckpointError::IncompatiblePreparedRevision)
    );
    assert_eq!(
        wrong_revision.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .next_occurrence_id(),
        wrong_revision_id
    );

    source.mutable_instruments[0]
        .as_mut()
        .unwrap()
        .begin_uncommitted_test_block();
    let mut empty = MutableRuntimeCheckpointStore::new();
    let empty_store_bytes = empty.declared_storage_bytes();
    assert_eq!(
        empty.capture_track(&source, track),
        Err(MutableRuntimeCheckpointError::UncommittedRuntime)
    );
    assert_eq!(empty.checkpoint_count(), 0);
    assert_eq!(empty.declared_storage_bytes(), empty_store_bytes);
    assert!(empty_store_bytes > 0);
}

#[test]
fn mutable_checkpoint_store_bounds_factory_calls_before_construction() {
    let counter = Arc::new(AtomicUsize::new(0));
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&counter)));
    MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE.with(|bytes| {
        bytes.set(Some(hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES));
    });
    let (revision, track) = mutable_checkpoint_fixture(8 << 32);
    let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    render_checkpoint_prefix(&mut playback, 1);
    let mut byte_limited = MutableRuntimeCheckpointStore::new();
    let byte_refusal = loop {
        match byte_limited.capture_track(&playback, track) {
            Ok(()) => {
                render_checkpoint_prefix(&mut playback, 1);
            }
            Err(error) => break error,
        }
    };
    assert_eq!(
        byte_refusal,
        MutableRuntimeCheckpointError::CheckpointBudgetExceeded
    );
    assert!(byte_limited.checkpoint_count() < MAX_MUTABLE_RUNTIME_CHECKPOINTS);
    assert!(byte_limited.declared_storage_bytes() <= MAX_MUTABLE_RUNTIME_CHECKPOINT_STORE_BYTES);
    let before_byte_refusal = counter.load(Ordering::Relaxed);
    assert_eq!(
        byte_limited.capture_track(&playback, track),
        Err(MutableRuntimeCheckpointError::CheckpointBudgetExceeded)
    );
    assert_eq!(counter.load(Ordering::Relaxed), before_byte_refusal);

    MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE.with(|bytes| bytes.set(None));
    // Minimize processor charge with the truthful zero-tail factory. A source
    // checkpoint also reserves its mandatory live ledger, so the byte ceiling
    // may be reached before the independent entry-count ceiling.
    let (revision, track) = mutable_checkpoint_fixture_with_release(8 << 32, 0);
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut source_bounded = MutableRuntimeCheckpointStore::new();
    let byte_refusal_with_source_ledger = loop {
        render_checkpoint_prefix(&mut playback, 1);
        let before_attempt = counter.load(Ordering::Relaxed);
        match source_bounded.capture_track(&playback, track) {
            Ok(()) => {}
            Err(error) => {
                assert_eq!(
                    error,
                    MutableRuntimeCheckpointError::CheckpointBudgetExceeded
                );
                assert_eq!(counter.load(Ordering::Relaxed), before_attempt);
                break error;
            }
        }
    };
    assert!(source_bounded.checkpoint_count() < MAX_MUTABLE_RUNTIME_CHECKPOINTS);
    let before_refusal = counter.load(Ordering::Relaxed);
    render_checkpoint_prefix(&mut playback, 1);
    assert_eq!(
        source_bounded.capture_track(&playback, track),
        Err(byte_refusal_with_source_ledger)
    );
    assert!(source_bounded.declared_storage_bytes() <= MAX_MUTABLE_RUNTIME_CHECKPOINT_STORE_BYTES);
    assert_eq!(counter.load(Ordering::Relaxed), before_refusal);
    source_bounded.clear();
    assert_eq!(source_bounded.checkpoint_count(), 0);
    assert_eq!(
        source_bounded.declared_storage_bytes(),
        MutableRuntimeCheckpointStore::new().declared_storage_bytes()
    );
    MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE.with(|bytes| bytes.set(None));
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = None);
}
