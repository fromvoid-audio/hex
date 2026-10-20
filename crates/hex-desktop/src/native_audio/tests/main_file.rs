use super::super::main_control_file::advance;
use hex_audio::AudioCandidatePreparationProgress;
use hex_engine::{
    AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageReservation,
    AudioStorageRole, OwnedAudioSource, SharedMetadata,
    native_device_resources::{
        OriginalResourceBankPlan, OriginalResourceInput, OriginalResourcePlanReservation,
        ResourceCatalogReservation,
    },
};
use hex_project::{BeatTime, Command, Created, Editor, Project, TrackDeviceSlot};
use hex_session::service::{MainControlFileJob, MainControlFileProgress};
use std::sync::atomic::AtomicBool;

#[test]
fn real_worker_token_main_file_reader_pending_is_nonfatal_and_stale_token_refuses() {
    assert!(exercise(false, false).is_some());
}

#[test]
fn outer_file_rows_cannot_be_smuggled_into_inner_compiler_original_ledger() {
    assert!(exercise(true, false).is_none());
}

#[test]
fn captured_main_file_continues_after_actual_publication_endpoint_retirement() {
    let reference = exercise(false, false).unwrap();
    let replaced = exercise(false, true).unwrap();
    assert_eq!(
        reference, replaced,
        "replacement must preserve exact captured PCM and encoding"
    );
}

fn exercise(
    include_unbound_file_rows: bool,
    retire_publication_after_block: bool,
) -> Option<Vec<u8>> {
    let _serial = super::super::tests::NATIVE_FIXTURE.lock().unwrap();
    let dir = super::super::tests::Files::new();
    let path = dir.0.join("actual-main-token.wav");
    let mut jobs = vec![MainControlFileJob::create_new_on_control(&path, 44_100, 513).unwrap()];
    // Reserve the actual adapter slot before constructing its captured parent.
    // This fixture remains a constituent test, not whole-host admission.
    let (events, _audio_events) = std::sync::mpsc::sync_channel(4);
    let mut file_owner = super::super::NativeAudioSink::new(events);
    if retire_publication_after_block {
        file_owner.main_files.try_reserve_exact(2).unwrap();
        file_owner
            .main_files
            .push(super::super::main_files::NativeMainFileJob {
                file: jobs.remove(0),
                continuation: None,
            });
    }
    let publication = hex_audio::AudioCandidatePublicationReservation {
        runtime_slots: 3,
        archive_slots: 3,
        member_rows: 16384,
        transfer_rows: 16384,
        receipt_rows: 8,
        data_rows: 4096,
        handle_rows: 4096,
        max_retained_data_bytes: 256 * 1024 * 1024,
        max_construction_peak_data_bytes: 256 * 1024 * 1024,
    };
    // Each prior fixture retires on the actual lifecycle lane. Serializing
    // callers does not synchronously complete that retirement or free its lease.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let (mut output, mut backend) = loop {
        let opened = if retire_publication_after_block {
            hex_audio::worker_fixture::open_with_reservation(44_100, publication)
        } else {
            hex_audio::worker_fixture::open(44_100)
        };
        match opened {
            Ok(output) => break output,
            Err(error) => {
                assert_eq!(error, "audio output is open or still shutting down");
                assert!(
                    std::time::Instant::now() < deadline,
                    "prior fixture did not retire"
                );
                std::thread::yield_now();
            }
        }
    };
    let storage = AudioStorageReservation {
        allocation_items: 256,
        handle_obligations: 128,
        max_prepared_data_bytes: 64 * 1024 * 1024,
        max_construction_data_bytes: 64 * 1024 * 1024,
        caller_retained_bytes: 0,
        caller_construction_bytes: 0,
    };
    let token = {
        let mut control = output.publication.control().unwrap();
        control
            .configure_audio_candidate_publication(publication)
            .unwrap();
        control
            .reserve_audio_candidate_preparation(
                hex_audio::AudioCandidatePreparationReservation {
                    source_capacity: 0,
                    storage,
                    member_rows: 4096,
                    transfer_rows: 4096,
                    handle_rows: 512,
                },
                Vec::new(),
            )
            .ok()
            .unwrap()
    };
    let client = output.take_client().unwrap();
    let mut editor = Editor::new(Project::new(44_100).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "main sampler".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_engine::empty_sampler_state_on_control().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let empty = editor.project().device(device).unwrap().state().clone();
    let assignment = hex_engine::sampler_replacement_state_on_control(&empty, 4).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            device,
            expected_state: empty,
            state: assignment.state,
            slot: assignment.slot,
            path: "main-sampler-original.wav".into(),
            source_sample_rate: 44_100,
            source_frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let target = SharedMetadata::new_on_control(editor.project().clone());
    editor.execute(Command::RemoveTrack(track)).unwrap();
    let source = OwnedAudioSource::try_resident_mono(44_100, vec![0.25, -0.5, 0.75, -1.0])
        .ok()
        .unwrap();
    let entries = vec![OriginalResourceInput::native(asset, &source)];
    let producer = OriginalResourceBankPlan::borrow(&entries);
    let allowance = OriginalResourcePlanReservation {
        inputs: AudioStorageReservation {
            allocation_items: 16,
            handle_obligations: 16,
            max_prepared_data_bytes: 65536,
            max_construction_data_bytes: 65536,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
        catalog: ResourceCatalogReservation {
            metadata_bytes: 4096,
            hash_frames: 4,
        },
        max_preparation_metadata_bytes: 4096,
        max_retained_data_bytes: 65536,
        max_construction_peak_bytes: 65536,
    };
    let signal = hex_engine::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
    let root = hex_engine::RetainedControlJob::try_from_owned_on_control(
        &mut Some(0u8),
        usize::MAX,
        |_: &mut u8| Ok::<_, ()>(hex_engine::RetainedControlJobProgress::Pending),
    )
    .unwrap();
    let worker = signal.bind_worker_on_control(root.route());
    let transforms = hex_engine::audio_transform::AudioTransformPreparation::with_retained_cancellation_on_control(worker.clone());
    let limits = AudioStorageLedgerReservation {
        data_rows: 1024,
        handle_rows: 1024,
        root_rows: 128,
        max_retained_bytes: 48 * 1024 * 1024,
        max_construction_peak_bytes: 48 * 1024 * 1024,
    };
    let mut original = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
    original
        .borrow_native_source_on_control(&source, AudioStorageRole::Caller)
        .unwrap();
    signal.visit_storage_on_control(&mut original).unwrap();
    worker.visit_storage_on_control(&mut original).unwrap();
    if include_unbound_file_rows {
        MainControlFileJob::visit_reserved_slots_storage_on_control(&jobs, &mut original).unwrap();
    }
    let mut reader = None;
    let mut identity = None;
    let mut compiler_error = None;
    let progress = output
        .publication
        .control()
        .unwrap()
        .prepare_audio_candidate_on_control(token, |mut operations| {
            operations
                .install_original_resources_on_control(
                    &producer,
                    allowance,
                    &AtomicBool::new(false),
                )
                .expect("actual original resource bank installer");
            if let Err(error) = operations.prepare_main_original_sampler_on_control(
                &target,
                &producer,
                &editor,
                &original,
                &transforms,
                limits,
            ) {
                compiler_error = Some(error);
                return Err(error);
            }
            identity = operations.prepared_identity();
            reader = Some(operations.register_compiled_reader_on_control()?);
            if retire_publication_after_block {
                Ok(())
            } else {
                operations.begin_main_control_work_on_control(513)
            }
        })
        .unwrap();
    if include_unbound_file_rows {
        // Required omission guard, NOT a passing whole-union construction.
        // Original desired-positive failure and source remain in evidence.
        assert_eq!(
            compiler_error,
            Some(hex_engine::AudioStorageError::SourceMismatch)
        );
        assert!(matches!(
            progress,
            AudioCandidatePreparationProgress::Refused(_)
        ));
        assert!(identity.is_none() && reader.is_none());
        assert!(!path.exists());
        drop(original);
        drop(client);
        backend.close();
        drop(output);
        return None;
    }
    assert_eq!(
        progress,
        if retire_publication_after_block {
            AudioCandidatePreparationProgress::Ready
        } else {
            AudioCandidatePreparationProgress::Collecting
        }
    );
    drop(original);
    let job = if retire_publication_after_block {
        &mut file_owner.main_files[0].file
    } else {
        &mut jobs[0]
    };
    job.bind_main_on_control(identity.unwrap()).unwrap();
    if retire_publication_after_block {
        let captured = output
            .publication
            .control()
            .unwrap()
            .begin_captured_main_file_on_control(token, identity.unwrap(), 513)
            .unwrap();
        file_owner.main_files[0].continuation = Some(captured);
        let super::super::main_files::NativeMainFileJob {
            file: job,
            continuation,
        } = &mut file_owner.main_files[0];
        let captured = continuation.as_mut().unwrap();
        let mut forbidden = false;
        assert!(
            output
                .publication
                .control()
                .unwrap()
                .prepare_audio_candidate_on_control(token, |_| {
                    forbidden = true;
                    Ok(())
                })
                .is_err()
        );
        assert!(
            !forbidden,
            "ordinary preparation cannot enter the obligated Main parent"
        );
        assert!(
            output
                .publication
                .control()
                .unwrap()
                .begin_captured_main_file_on_control(token, identity.unwrap(), 513)
                .is_err()
        );
        let mut callback = [[0.0; 2]; 32];
        let measured = allocation_counter::measure(|| backend.render(&mut callback).unwrap());
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let mut turn = |job: &mut MainControlFileJob| {
            let _ = captured.poll_retirement_on_control();
            job.advance_on_control(|continuation| {
                let mut loan = captured
                    .try_loan_on_control()
                    .map_err(|error| match error {
                        hex_audio::CapturedMainFileError::Busy => {
                            hex_session::service::MainControlHostError::Busy
                        }
                        _ => hex_session::service::MainControlHostError::Refused,
                    })?;
                match continuation.run(&mut loan) {
                    Ok(()) | Err(hex_engine::AudioStorageError::ExternalReadersPending) => Ok(()),
                    Err(_) => Err(hex_session::service::MainControlHostError::Refused),
                }
            })
        };
        assert!(matches!(
            turn(job).unwrap(),
            MainControlFileProgress::Progress { .. }
        ));
        // Output replacement drops this same WorkerControl, whose Drop closes
        // the publication endpoint. Keep the captured File job and its exact
        // preparation token alive: a later turn must still render its target.
        output.publication.close();
        backend.close();
        drop(output); // actual StreamOwner retirement, not a caller join flag
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (replacement, mut replacement_backend) = loop {
            match hex_audio::worker_fixture::open(44_100) {
                Ok(output) => break output,
                Err(error) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "native frontier did not release lease while captured DATA remained: {error}"
                    );
                    std::thread::yield_now();
                }
            }
        };
        match turn(job) {
            Ok(MainControlFileProgress::Progress { .. }) => {}
            Err(error) => panic!("captured export lost its actual parent: {error:?}"),
            Ok(_) => panic!("captured export made no progress after endpoint retirement"),
        }
        assert!(matches!(
            turn(job).unwrap(),
            MainControlFileProgress::Progress { .. }
        ));
        assert!(matches!(turn(job).unwrap(), MainControlFileProgress::Busy));
        assert!(
            !path.exists(),
            "actual retained reader must prevent final publication"
        );
        drop(reader.take());
        assert!(
            matches!(turn(job).unwrap(), MainControlFileProgress::Published(report) if report.frames==513)
        );
        assert!(
            turn(job).is_err(),
            "successful publication cannot run twice"
        );
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 44 + 513 * 4);
        assert!(bytes[44..].iter().any(|byte| *byte != 0));
        drop(turn);
        assert!(matches!(
            captured.try_loan_on_control(),
            Err(hex_audio::CapturedMainFileError::Stale)
        ));
        drop(client);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !file_owner.main_files.is_empty() {
            file_owner.poll_main_file_retirement_on_control();
            assert!(
                std::time::Instant::now() < deadline,
                "actual persistent File job failed normal DATA retirement"
            );
            std::thread::yield_now();
        }
        replacement_backend.close();
        drop(replacement);
        return Some(bytes);
    }
    for _ in 0..3 {
        assert!(matches!(
            advance(&mut output, token, job).unwrap(),
            MainControlFileProgress::Progress { .. }
        ));
    }
    // Actual finish returns reader-Pending. This must NOT poison the host's
    // preparation into refusal/retirement: repeatedly invoke SAME token/root.
    for _ in 0..3 {
        assert!(matches!(
            advance(&mut output, token, job).unwrap(),
            MainControlFileProgress::Busy
        ));
        assert!(!path.exists());
        assert_eq!(
            output
                .publication
                .control()
                .unwrap()
                .prepare_audio_candidate_on_control(token, |_| Ok(()))
                .unwrap(),
            AudioCandidatePreparationProgress::Collecting
        );
    }
    drop(reader.take());
    assert!(
        matches!(advance(&mut output,token,job).unwrap(),MainControlFileProgress::Published(report) if report.frames==513)
    );
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 44 + 513 * 4);
    assert!(bytes[44..].iter().any(|byte| *byte != 0));
    assert!(
        output
            .publication
            .control()
            .unwrap()
            .cancel_audio_candidate_preparation(token)
    );
    // The old token cannot execute a caller closure on a cancelled incarnation.
    let mut invoked = false;
    let result = output
        .publication
        .control()
        .unwrap()
        .prepare_audio_candidate_on_control(token, |_| {
            invoked = true;
            Ok(())
        });
    assert!(!invoked);
    assert!(matches!(
        result,
        Ok(AudioCandidatePreparationProgress::Retiring) | Err(_)
    ));
    drop(client);
    backend.close();
    drop(output);
    Some(bytes)
}
