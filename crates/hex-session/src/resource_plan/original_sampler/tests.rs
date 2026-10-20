//! Owning producer component controls. Direct producer calls here deliberately
//! do NOT claim the full native-host prefix or normal-Service admission.
use super::*;
use hex_engine::sampler_host_fields::OriginalSamplerPendingSession;

fn retained_session() -> (
    Session,
    hex_engine::RetainedControlCancellationSignal,
    hex_engine::RetainedControlJob,
) {
    let signal = hex_engine::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
    let job = hex_engine::RetainedControlJob::try_from_owned_on_control(
        &mut Some(0_u8),
        usize::MAX,
        |_: &mut u8| Ok::<_, ()>(hex_engine::RetainedControlJobProgress::Pending),
    )
    .unwrap();
    let session = Session::new_with_retained_cancellation_on_control(
        48_000,
        8,
        0,
        signal.bind_worker_on_control(job.route()),
    )
    .unwrap();
    (session, signal, job)
}
fn context() -> hex_engine::AudioSourceContext {
    hex_engine::AudioSourceContext {
        project_generation: 3,
        editor_revision: 1,
    }
}
fn limits() -> hex_engine::AudioStorageLedgerReservation {
    hex_engine::AudioStorageLedgerReservation {
        data_rows: 512,
        handle_rows: 128,
        root_rows: 16,
        max_retained_bytes: 8_000_000,
        max_construction_peak_bytes: 8_000_000,
    }
}
fn request() -> Option<OriginalSamplerTransitionRequest> {
    Some(OriginalSamplerTransitionRequest::command(
        Command::MasterGain { gain: 0.5 },
    ))
}
fn stage_component(loan: &mut OriginalSamplerSessionLoan<'_>) -> bool {
    OriginalSamplerPendingSession::stage_and_capture_on_control(loan).unwrap()
}

#[test]
fn pending_pair_retains_actual_mono_stereo_media_and_old_captured_source() {
    use hex_engine::OwnedAudioSource;
    use hex_project::Created;
    for stereo in [false, true] {
        let (mut session, _signal, _job) = retained_session();
        let source = if stereo {
            OwnedAudioSource::try_resident(48_000, vec![[0.25, -0.5]; 4])
                .ok()
                .unwrap()
        } else {
            OwnedAudioSource::try_resident_mono(48_000, vec![0.25; 4])
                .ok()
                .unwrap()
        };
        let prepared = crate::PreparedImport::from_native_source(
            "actual-pending-source.wav".into(),
            source,
            48_000,
            hex_media::ResampleLimits {
                max_source_frames: 4,
                max_output_frames: 4,
            },
        )
        .unwrap();
        let Created::Asset(asset) = session
            .editor
            .execute(Command::AddAsset {
                path: "actual-pending-source.wav".into(),
                sample_rate: 48_000,
                frames: 4,
            })
            .unwrap()
        else {
            panic!("actual test asset")
        };
        session
            .media
            .try_insert_on_control(
                asset,
                crate::ResidentAsset {
                    source_audio: prepared.source_audio,
                    audio: Some(prepared.audio),
                    waveform: crate::SharedWaveform::new_on_control(prepared.waveform),
                    frames: 4,
                },
                hex_project::MAX_ASSETS,
            )
            .unwrap();
        let previous_project = crate::SharedProject::new_on_control(session.project().clone());
        let mut previous_bindings = SortedEntries::new();
        previous_bindings
            .try_insert_on_control(
                asset,
                crate::AudioSourceBinding::Resident(session.audio(asset).unwrap().clone()),
                hex_project::MAX_ASSETS,
            )
            .unwrap();
        let mut previous_bindings = Some(previous_bindings);
        let previous = crate::service::SharedCanonicalAudioSource::new_on_control(
            crate::service::canonical_audio_source::CanonicalAudioSourceSnapshot::from_pending_original_on_control(
                &previous_project,
                hex_engine::AudioSourceContext { project_generation: 3, editor_revision: 0 },
                &mut previous_bindings,
            ).unwrap(),
        );
        drop(previous_project);
        let original_audio = session.audio(asset).unwrap().frames().as_ptr();
        let initial = session.project().clone();
        let mut request = request();
        {
            let mut loan = session
                .original_sampler_pending_session_loan_on_control(&mut request, context())
                .unwrap();
            assert!(stage_component(&mut loan));
            assert_eq!(loan.session.project(), &initial);
            let source = loan.pending_source.source.as_ref().unwrap();
            assert_eq!(source.project_owner_on_control().assets().count(), 1);
            let captured = crate::service::sampler_captured_source_fields_on_control(source);
            assert_eq!(
                captured.payload.resident(0).unwrap().frames().as_ptr(),
                original_audio
            );
            loan.with_inventory(|inventory| {
                let collect = |budget| {
                    let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
                    inventory.visit_worker_model_media_storage_on_control(&mut ledger)?;
                    assert_eq!(
                        ledger.validate_selected_shared_handles_on_control(),
                        Err(AudioStorageError::UnknownStorage),
                        "the actual old captured source still owns a PCM alias"
                    );
                    crate::service::visit_canonical_source_selected_storage_on_control(
                        &previous,
                        &mut ledger,
                    )?;
                    ledger.validate_selected_shared_handles_on_control()?;
                    let measured = ledger.summary()?;
                    inventory.visit_worker_model_media_storage_on_control(&mut ledger)?;
                    assert_eq!(ledger.summary()?, measured);
                    Ok::<_, AudioStorageError>(measured)
                };
                let measured = collect(limits())?;
                let mut exact = limits();
                exact.max_retained_bytes = measured.retained_bytes;
                exact.max_construction_peak_bytes = measured.construction_peak_bytes;
                assert_eq!(collect(exact)?, measured);
                for retained in [true, false] {
                    let mut short = exact;
                    if retained {
                        short.max_retained_bytes -= 1;
                    } else {
                        short.max_construction_peak_bytes -= 1;
                    }
                    assert!(collect(short).is_err());
                }
                // This is a selected after-birth report bound. It does not
                // establish the closed prefix or whole Service admission.
                assert!(measured.require_complete().is_err());
                Ok(())
            })
            .unwrap();
        }
        session.clear_original_sampler_pending_on_control().unwrap();
        assert_eq!(session.project(), &initial);
        assert_eq!(
            session.audio(asset).unwrap().frames().as_ptr(),
            original_audio
        );
        assert_eq!(previous.project_owner_on_control().assets().count(), 1);
    }
}

#[test]
fn pending_pair_is_persistent_and_both_actual_project_handles_are_mandatory() {
    let (mut session, _signal, _job) = retained_session();
    let initial = session.project().clone();
    let mut request = request();
    {
        let mut loan = session
            .original_sampler_pending_session_loan_on_control(&mut request, context())
            .unwrap();
        assert!(request.is_none());
        assert!(loan.pending_source.project.is_none());
        assert!(stage_component(&mut loan));
        assert_eq!(loan.session.project(), &initial);
        let project = loan.pending_source.project.as_ref().unwrap();
        let source = loan.pending_source.source.as_ref().unwrap();
        assert!(project.shares_owner_with(source.project_owner_on_control()));
        assert!(!std::ptr::eq(project, source.project_owner_on_control()));
        assert_eq!(source.context(), context());
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        ledger
            .borrow_sampler_captured_source_fields_on_control(
                &crate::service::sampler_captured_source_fields_on_control(source),
            )
            .unwrap();
        assert_eq!(
            ledger.validate_selected_shared_handles_on_control(),
            Err(AudioStorageError::UnknownStorage)
        );
        crate::shared_project::visit_shared_project_selected_storage_on_control(
            project,
            &mut ledger,
        )
        .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert_eq!(ledger.selected_shared_counts_on_control(), (2, 3));
        loan.with_inventory(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits())?;
            inventory.visit_worker_model_media_storage_on_control(&mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()
        })
        .unwrap();
    }
    let pending_pointer = session
        .original_sampler_source
        .get_mut()
        .source
        .as_ref()
        .unwrap()
        .project_owner_on_control()
        .as_ref() as *const _;
    assert!(
        session
            .commit_original_sampler_pending_on_control()
            .unwrap()
            .changed
    );
    assert_ne!(session.project(), &initial);
    assert_eq!(
        pending_pointer,
        session
            .original_sampler_source
            .get_mut()
            .source
            .as_ref()
            .unwrap()
            .project_owner_on_control()
            .as_ref() as *const _
    );
    session.clear_original_sampler_pending_on_control().unwrap();
    assert!(session.original_sampler_source.get_mut().vacant());
    assert!(session.editor.can_undo());
}

#[test]
fn pending_noop_and_undo_redo_keep_canonical_ids_history_and_pair_correlation() {
    let (mut session, _signal, _job) = retained_session();
    let initial = session.project().clone();
    for undo in [false, true, false] {
        let mut request = if undo {
            Some(OriginalSamplerTransitionRequest::undo())
        } else if session.editor.can_redo() {
            Some(OriginalSamplerTransitionRequest::redo())
        } else {
            request()
        };
        {
            let mut loan = session
                .original_sampler_pending_session_loan_on_control(&mut request, context())
                .unwrap();
            assert!(stage_component(&mut loan));
        }
        assert!(
            session
                .commit_original_sampler_pending_on_control()
                .unwrap()
                .changed
        );
        session.clear_original_sampler_pending_on_control().unwrap();
    }
    let changed = session.project().clone();
    assert_ne!(initial, changed);
    let mut request = Some(OriginalSamplerTransitionRequest::command(
        Command::ClearLoopRegion,
    ));
    {
        let mut loan = session
            .original_sampler_pending_session_loan_on_control(&mut request, context())
            .unwrap();
        assert!(!stage_component(&mut loan));
        assert!(loan.pending_source.project.is_none());
        assert!(loan.pending_source.source.is_none());
    }
    assert!(
        !session
            .commit_original_sampler_pending_on_control()
            .unwrap()
            .changed
    );
    session.clear_original_sampler_pending_on_control().unwrap();
    assert_eq!(session.project(), &changed);
    assert!(session.editor.can_undo());
}

#[test]
fn busy_metadata_waveform_and_transform_return_the_same_unprepared_request() {
    let (session, _signal, _job) = retained_session();
    for kind in 0..3 {
        let metadata =
            (kind == 0).then(|| session.original_sampler_source.try_borrow_mut().unwrap());
        let waveform = (kind == 1).then(|| session.audio_clip_waveforms.try_borrow_mut().unwrap());
        let transform = (kind == 2).then(|| {
            session
                .audio_transforms
                .original_sampler_transaction_on_control()
                .unwrap()
        });
        let mut name = String::with_capacity(128);
        name.push_str("untouched producer request");
        let original = name.as_ptr();
        let mut request = Some(OriginalSamplerTransitionRequest::command(
            Command::AddTrack { name },
        ));
        assert!(matches!(
            session.original_sampler_pending_session_loan_on_control(&mut request, context()),
            Err(AudioStorageError::ExternalReadersPending)
        ));
        assert!(
            matches!(request.as_ref().unwrap().command_value(), Some(Command::AddTrack { name }) if name.as_ptr() == original && name.capacity() == 128)
        );
        session
            .editor
            .require_no_original_sampler_pending_on_control()
            .unwrap();
        drop((metadata, waveform, transform));
        assert!(
            session
                .original_sampler_source
                .try_borrow()
                .unwrap()
                .vacant()
        );
    }
}

#[test]
fn pending_foreign_equal_project_and_context_refuse_without_adoption() {
    let (mut session, _signal, _job) = retained_session();
    let mut request = request();
    {
        let mut loan = session
            .original_sampler_pending_session_loan_on_control(&mut request, context())
            .unwrap();
        stage_component(&mut loan);
        let actual = loan.pending_source.project.take().unwrap();
        loan.pending_source.project = Some(crate::SharedProject::new_on_control(
            actual.as_ref().clone(),
        ));
        assert!(matches!(
            loan.pending_source.inspect(),
            Err(AudioStorageError::SourceMismatch)
        ));
        loan.pending_source.project = Some(actual);
        loan.pending_source
            .context
            .as_mut()
            .unwrap()
            .editor_revision += 1;
        assert!(matches!(
            loan.pending_source.inspect(),
            Err(AudioStorageError::SourceMismatch)
        ));
        loan.pending_source.context = Some(context());
        loan.pending_source.inspect().unwrap();
    }
    session.clear_original_sampler_pending_on_control().unwrap();
    assert!(session.original_sampler_source.get_mut().vacant());
    assert_eq!(session.project().master_gain(), 1.0);
}

#[test]
fn pending_cleanup_fault_keeps_first_payload_pair_and_once_only_consumption() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (mut session, _signal, _job) = retained_session();
    let initial = session.project().clone();
    let mut request = request();
    {
        let mut loan = session
            .original_sampler_pending_session_loan_on_control(&mut request, context())
            .unwrap();
        stage_component(&mut loan);
    }
    let calls = Arc::new(AtomicUsize::new(0));
    session.original_sampler_source.get_mut().cleanup_probe =
        Some(PendingCleanupProbe(calls.clone()));
    assert_eq!(
        session.clear_original_sampler_pending_on_control(),
        Err(AudioStorageError::UnknownStorage)
    );
    let source = session.original_sampler_source.get_mut();
    let first = std::ptr::from_ref(source.first_panic.as_ref().unwrap().as_ref());
    assert!(source.project.is_some() && source.source.is_some());
    assert!(source.cleanup_probe.is_none());
    for _ in 0..3 {
        assert_eq!(
            session.clear_original_sampler_pending_on_control(),
            Err(AudioStorageError::UnknownStorage)
        );
        assert!(std::ptr::eq(
            first,
            std::ptr::from_ref(
                session
                    .original_sampler_source
                    .get_mut()
                    .first_panic
                    .as_ref()
                    .unwrap()
                    .as_ref()
            )
        ));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(session.project(), &initial);
    assert!(session.original_sampler_session_loan_on_control().is_err());
}
